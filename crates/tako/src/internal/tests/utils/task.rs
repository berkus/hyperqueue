use super::resources::ResBuilder;
use crate::internal::common::resources::{NumOfNodes, ResourceAmount, ResourceId};
use crate::internal::messages::worker::TaskRunningMsg;
use crate::internal::server::task::{Task, TaskConfiguration, TaskInput};
use crate::{Priority, TaskId};
use std::rc::Rc;

pub struct TaskBuilder {
    id: TaskId,
    inputs: Vec<TaskInput>,
    n_outputs: u32,
    resources: ResBuilder,
    user_priority: Priority,
    crash_limit: u32,
}

impl TaskBuilder {
    pub fn new<T: Into<TaskId>>(id: T) -> TaskBuilder {
        TaskBuilder {
            id: id.into(),
            inputs: Default::default(),
            n_outputs: 0,
            resources: Default::default(),
            user_priority: 0,
            crash_limit: 5,
        }
    }

    pub fn user_priority(mut self, value: Priority) -> TaskBuilder {
        self.user_priority = value;
        self
    }

    pub fn simple_deps(mut self, deps: &[&Task]) -> TaskBuilder {
        self.inputs = deps.iter().map(|&tr| TaskInput::new(tr.id, 0)).collect();
        self
    }

    pub fn task_deps(mut self, deps: &[&Task]) -> TaskBuilder {
        self.inputs = deps
            .iter()
            .map(|&tr| TaskInput::new_task_dependency(tr.id))
            .collect();
        self
    }

    pub fn outputs(mut self, value: u32) -> TaskBuilder {
        self.n_outputs = value;
        self
    }

    pub fn resources(mut self, resources: ResBuilder) -> TaskBuilder {
        self.resources = resources;
        self
    }

    pub fn n_nodes(mut self, count: NumOfNodes) -> TaskBuilder {
        self.resources = self.resources.n_nodes(count);
        self
    }

    pub fn cpus_compact(mut self, count: ResourceAmount) -> TaskBuilder {
        self.resources = self.resources.cpus(count);
        self
    }

    pub fn time_request(mut self, time_s: u64) -> TaskBuilder {
        self.resources = self.resources.min_time_secs(time_s);
        self
    }

    pub fn add_resource<Id: Into<ResourceId>>(
        mut self,
        id: Id,
        amount: ResourceAmount,
    ) -> TaskBuilder {
        self.resources = self.resources.add(id, amount);
        self
    }

    pub fn build(self) -> Task {
        let resources = self.resources.finish();
        resources.validate().unwrap();
        Task::new(
            self.id,
            self.inputs.into_iter().collect(),
            Rc::new(TaskConfiguration {
                resources,
                n_outputs: self.n_outputs,
                time_limit: None,
                user_priority: self.user_priority,
                crash_limit: self.crash_limit,
            }),
            Default::default(),
            false,
            false,
        )
    }
}

pub fn task<T: Into<TaskId>>(id: T) -> Task {
    TaskBuilder::new(id.into()).outputs(1).build()
}

pub fn task_with_deps<T: Into<TaskId>>(id: T, deps: &[&Task], n_outputs: u32) -> Task {
    TaskBuilder::new(id.into())
        .simple_deps(deps)
        .outputs(n_outputs)
        .build()
}

pub fn task_running_msg<T: Into<TaskId>>(task_id: T) -> TaskRunningMsg {
    TaskRunningMsg {
        id: task_id.into(),
        context: Default::default(),
    }
}
