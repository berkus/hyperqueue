use std::rc::Rc;
use std::sync::Arc;

use futures::{Sink, SinkExt, Stream, StreamExt};
use orion::kdf::SecretKey;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Notify};

use tako::messages::gateway::{
    CancelTasks, FromGatewayMessage, StopWorkerRequest, ToGatewayMessage,
};

use crate::client::status::{job_status, Status};
use crate::common::serverdir::ServerDir;
use crate::server::event::MonitoringEvent;
use crate::server::job::JobTaskCounters;
use crate::server::rpc::Backend;
use crate::server::state::{State, StateRef};
use crate::stream::server::control::StreamServerControlMessage;
use crate::transfer::connection::ServerConnection;
use crate::transfer::messages::WaitForJobsResponse;
use crate::transfer::messages::{
    CancelJobResponse, FromClientMessage, IdSelector, JobDetail, JobInfoResponse, StatsResponse,
    StopWorkerResponse, TaskSelector, ToClientMessage, WorkerListResponse,
};
use crate::{JobId, JobTaskCount, WorkerId};

pub mod autoalloc;
mod submit;

pub async fn handle_client_connections(
    state_ref: StateRef,
    tako_ref: Backend,
    server_dir: ServerDir,
    listener: TcpListener,
    end_flag: Rc<Notify>,
    key: Arc<SecretKey>,
) {
    while let Ok((connection, _)) = listener.accept().await {
        let state_ref = state_ref.clone();
        let tako_ref = tako_ref.clone();
        let end_flag = end_flag.clone();
        let key = key.clone();
        let server_dir = server_dir.clone();

        // TODO: remove this spawn
        tokio::task::spawn_local(async move {
            if let Err(e) =
                handle_client(connection, server_dir, state_ref, tako_ref, end_flag, key).await
            {
                log::error!("Client error: {}", e);
            }
        });
    }
}

async fn handle_client(
    socket: TcpStream,
    server_dir: ServerDir,
    state_ref: StateRef,
    tako_ref: Backend,
    end_flag: Rc<Notify>,
    key: Arc<SecretKey>,
) -> crate::Result<()> {
    log::debug!("New client connection");
    let socket = ServerConnection::accept_client(socket, key).await?;
    let (tx, rx) = socket.split();

    client_rpc_loop(tx, rx, server_dir, state_ref, tako_ref, end_flag).await;
    log::debug!("Client connection ended");
    Ok(())
}

pub async fn client_rpc_loop<
    Tx: Sink<ToClientMessage> + Unpin,
    Rx: Stream<Item = crate::Result<FromClientMessage>> + Unpin,
>(
    mut tx: Tx,
    mut rx: Rx,
    server_dir: ServerDir,
    state_ref: StateRef,
    tako_ref: Backend,
    end_flag: Rc<Notify>,
) {
    while let Some(message_result) = rx.next().await {
        match message_result {
            Ok(message) => {
                let response = match message {
                    FromClientMessage::Submit(msg) => {
                        submit::handle_submit(&state_ref, &tako_ref, msg).await
                    }
                    FromClientMessage::JobInfo(msg) => compute_job_info(&state_ref, &msg.selector),
                    FromClientMessage::Resubmit(msg) => {
                        submit::handle_resubmit(&state_ref, &tako_ref, msg).await
                    }
                    FromClientMessage::Stop => {
                        end_flag.notify_one();
                        break;
                    }
                    FromClientMessage::WorkerList => handle_worker_list(&state_ref).await,
                    FromClientMessage::WorkerInfo(msg) => {
                        handle_worker_info(&state_ref, msg.worker_id).await
                    }
                    FromClientMessage::StopWorker(msg) => {
                        handle_worker_stop(&state_ref, &tako_ref, msg.selector).await
                    }
                    FromClientMessage::Cancel(msg) => {
                        handle_job_cancel(&state_ref, &tako_ref, &msg.selector).await
                    }
                    FromClientMessage::JobDetail(msg) => {
                        compute_job_detail(&state_ref, msg.job_id_selector, msg.task_selector)
                    }
                    FromClientMessage::Stats => compose_server_stats(&state_ref, &tako_ref).await,
                    FromClientMessage::AutoAlloc(msg) => {
                        autoalloc::handle_autoalloc_message(&server_dir, &state_ref, msg).await
                    }
                    FromClientMessage::WaitForJobs(msg) => {
                        handle_wait_for_jobs_message(&state_ref, msg.selector).await
                    }
                    FromClientMessage::MonitoringEvents(request) => {
                        let events: Vec<MonitoringEvent> = state_ref
                            .get()
                            .event_storage()
                            .get_events_after(request.after_id.unwrap_or(0))
                            .cloned()
                            .collect();
                        ToClientMessage::MonitoringEventsResponse(events)
                    }
                };
                assert!(tx.send(response).await.is_ok());
            }
            Err(e) => {
                log::error!("Cannot parse client message: {}", e);
                if tx
                    .send(ToClientMessage::Error(format!(
                        "Cannot parse message: {}",
                        e
                    )))
                    .await
                    .is_err()
                {
                    log::error!(
                        "Cannot send error response to client, it has probably disconnected."
                    );
                }
            }
        }
    }
}

/// Waits until all jobs matched by the `selector` are finished (either by completing successfully,
/// failing or being canceled).
async fn handle_wait_for_jobs_message(
    state_ref: &StateRef,
    selector: IdSelector,
) -> ToClientMessage {
    let update_counters = |response: &mut WaitForJobsResponse, counters: &JobTaskCounters| {
        if counters.n_canceled_tasks > 0 {
            response.canceled += 1;
        } else if counters.n_failed_tasks > 0 {
            response.failed += 1;
        } else {
            response.finished += 1;
        }
    };

    let (receivers, mut response) = {
        let mut state = state_ref.get_mut();
        let job_ids: Vec<JobId> = get_job_ids(&state, &selector);

        let mut response = WaitForJobsResponse::default();
        let mut receivers = vec![];

        for job_id in job_ids {
            match state.get_job_mut(job_id) {
                Some(job) => {
                    if job.is_terminated() {
                        update_counters(&mut response, &job.counters);
                    } else {
                        let rx = job.subscribe_to_completion();
                        receivers.push(rx);
                    }
                }
                None => response.invalid += 1,
            }
        }
        (receivers, response)
    };

    let results = futures::future::join_all(receivers).await;
    let state = state_ref.get();

    for result in results {
        match result {
            Ok(job_id) => {
                match state.get_job(job_id) {
                    Some(job) => update_counters(&mut response, &job.counters),
                    None => continue,
                };
            }
            Err(err) => log::error!("Error while waiting on job(s): {:?}", err),
        };
    }

    ToClientMessage::WaitForJobsResponse(response)
}

async fn handle_worker_stop(
    state_ref: &StateRef,
    tako_ref: &Backend,
    selector: IdSelector,
) -> ToClientMessage {
    log::debug!("Client asked for worker termination {:?}", selector);
    let mut responses: Vec<(WorkerId, StopWorkerResponse)> = Vec::new();

    let worker_ids: Vec<WorkerId> = match selector {
        IdSelector::Specific(array) => array.iter().map(|id| id.into()).collect(),
        IdSelector::All => state_ref
            .get()
            .get_workers()
            .iter()
            .filter(|(_, worker)| worker.make_info().ended.is_none())
            .map(|(_, worker)| worker.worker_id())
            .collect(),
        IdSelector::LastN(n) => {
            let mut ids: Vec<_> = state_ref.get().get_workers().keys().copied().collect();
            ids.sort_by_key(|&k| std::cmp::Reverse(k));
            ids.truncate(n as usize);
            ids
        }
    };

    for worker_id in worker_ids {
        if let Some(worker) = state_ref.get().get_worker(worker_id) {
            if worker.make_info().ended.is_some() {
                responses.push((worker_id, StopWorkerResponse::AlreadyStopped));
                continue;
            }
        } else {
            responses.push((worker_id, StopWorkerResponse::InvalidWorker));
            continue;
        }
        let response = tako_ref
            .clone()
            .send_tako_message(FromGatewayMessage::StopWorker(StopWorkerRequest {
                worker_id,
            }))
            .await;

        match response {
            Ok(result) => match result {
                ToGatewayMessage::WorkerStopped => {
                    responses.push((worker_id, StopWorkerResponse::Stopped))
                }
                ToGatewayMessage::Error(error) => {
                    responses.push((worker_id, StopWorkerResponse::Failed(error.message)))
                }
                msg => panic!(
                    "Received invalid response to worker: {} stop: {:?}",
                    worker_id, msg
                ),
            },
            Err(err) => {
                responses.push((worker_id, StopWorkerResponse::Failed(err.to_string())));
                log::error!("Unable to stop worker: {} error: {:?}", worker_id, err);
            }
        }
    }
    ToClientMessage::StopWorkerResponse(responses)
}

fn compute_job_detail(
    state_ref: &StateRef,
    job_id_selector: IdSelector,
    task_selector: Option<TaskSelector>,
) -> ToClientMessage {
    let state = state_ref.get();

    let job_ids: Vec<JobId> = get_job_ids(&state, &job_id_selector);

    let mut responses: Vec<(JobId, Option<JobDetail>)> = Vec::new();
    for job_id in job_ids {
        let opt_detail = state
            .get_job(job_id)
            .map(|j| j.make_job_detail(task_selector.as_ref()));

        if let Some(detail) = opt_detail {
            responses.push((job_id, Some(detail)));
        } else {
            responses.push((job_id, None));
        }
    }
    ToClientMessage::JobDetailResponse(responses)
}

fn get_job_ids(state: &State, selector: &IdSelector) -> Vec<JobId> {
    match &selector {
        IdSelector::All => state.jobs().map(|job| job.job_id).collect(),
        IdSelector::LastN(n) => state.last_n_ids(*n).collect(),
        IdSelector::Specific(array) => array.iter().map(|id| id.into()).collect(),
    }
}

async fn compose_server_stats(_state_ref: &StateRef, backend: &Backend) -> ToClientMessage {
    let stream_stats = {
        let (sender, receiver) = oneshot::channel();
        backend.send_stream_control(StreamServerControlMessage::Stats(sender));
        receiver.await.unwrap()
    };
    ToClientMessage::StatsResponse(StatsResponse { stream_stats })
}

fn compute_job_info(state_ref: &StateRef, selector: &IdSelector) -> ToClientMessage {
    let state = state_ref.get();

    let jobs: Vec<_> = match selector {
        IdSelector::All => state.jobs().map(|j| j.make_job_info()).collect(),
        IdSelector::LastN(n) => state
            .last_n_ids(*n)
            .filter_map(|id| state.get_job(id))
            .map(|j| j.make_job_info())
            .collect(),
        IdSelector::Specific(array) => array
            .iter()
            .filter_map(|id| state.get_job(JobId::new(id)))
            .map(|j| j.make_job_info())
            .collect(),
    };
    ToClientMessage::JobInfoResponse(JobInfoResponse { jobs })
}

async fn handle_job_cancel(
    state_ref: &StateRef,
    tako_ref: &Backend,
    selector: &IdSelector,
) -> ToClientMessage {
    let job_ids: Vec<JobId> = match selector {
        IdSelector::All => state_ref
            .get()
            .jobs()
            .map(|job| job.make_job_info())
            .filter(|job_info| matches!(job_status(job_info), Status::Waiting | Status::Running))
            .map(|job_info| job_info.id)
            .collect(),
        IdSelector::LastN(n) => state_ref.get().last_n_ids(*n).collect(),
        IdSelector::Specific(array) => array.iter().map(|id| id.into()).collect(),
    };

    let mut responses: Vec<(JobId, CancelJobResponse)> = Vec::new();
    for job_id in job_ids {
        let tako_task_ids;
        {
            let n_tasks = match state_ref.get().get_job(job_id) {
                None => {
                    responses.push((job_id, CancelJobResponse::InvalidJob));
                    continue;
                }
                Some(job) => {
                    tako_task_ids = job.non_finished_task_ids();
                    job.n_tasks()
                }
            };
            if tako_task_ids.is_empty() {
                responses.push((job_id, CancelJobResponse::Canceled(Vec::new(), n_tasks)));
                continue;
            }
        }

        let canceled_tasks = match tako_ref
            .send_tako_message(FromGatewayMessage::CancelTasks(CancelTasks {
                tasks: tako_task_ids,
            }))
            .await
            .unwrap()
        {
            ToGatewayMessage::CancelTasksResponse(msg) => msg.cancelled_tasks,
            ToGatewayMessage::Error(msg) => {
                responses.push((job_id, CancelJobResponse::Failed(msg.message)));
                continue;
            }
            _ => panic!("Invalid message"),
        };

        let mut state = state_ref.get_mut();
        let job = state.get_job_mut(job_id).unwrap();
        let canceled_ids: Vec<_> = canceled_tasks
            .iter()
            .map(|tako_id| job.set_cancel_state(*tako_id, tako_ref))
            .collect();
        let already_finished = job.n_tasks() - canceled_ids.len() as JobTaskCount;
        responses.push((
            job_id,
            CancelJobResponse::Canceled(canceled_ids, already_finished),
        ));
    }

    ToClientMessage::CancelJobResponse(responses)
}

async fn handle_worker_list(state_ref: &StateRef) -> ToClientMessage {
    let state = state_ref.get();

    ToClientMessage::WorkerListResponse(WorkerListResponse {
        workers: state
            .get_workers()
            .values()
            .map(|w| w.make_info())
            .collect(),
    })
}

async fn handle_worker_info(state_ref: &StateRef, worker_id: WorkerId) -> ToClientMessage {
    let state = state_ref.get();

    ToClientMessage::WorkerInfoResponse(state.get_worker(worker_id).map(|w| w.make_info()))
}