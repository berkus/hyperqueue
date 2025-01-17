use crate::internal::server::task::Task;
use crate::internal::server::taskmap::TaskMap;
use crate::{PriorityTuple, TaskId};
use priority_queue::PriorityQueue;

#[derive(Default)]
pub(crate) struct MultiNodeQueue {
    pub(crate) queue: PriorityQueue<TaskId, PriorityTuple>,
}

fn task_priority_tuple(task: &Task) -> PriorityTuple {
    (
        task.configuration.user_priority,
        task.get_scheduler_priority(),
    )
}

impl MultiNodeQueue {
    pub fn recompute_priorities(&mut self, task_map: &TaskMap) {
        if self.queue.is_empty() {
            return;
        }
        let new_queue = PriorityQueue::with_capacity(self.queue.len());
        let old_queue = std::mem::replace(&mut self.queue, new_queue);
        for (task_id, _) in old_queue {
            let task = task_map.get_task(task_id);
            self.add_task(task)
        }
    }

    pub fn add_task(&mut self, task: &Task) {
        self.queue.push(task.id, task_priority_tuple(task));
    }

    pub fn remove_task(&mut self, task_id: TaskId) {
        self.queue.remove(&task_id);
    }

    pub fn all_tasks(&self) -> impl Iterator<Item = &TaskId> {
        self.queue.iter().map(|x| x.0)
    }
}
