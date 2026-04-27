use std::collections::VecDeque;

pub enum Step<S, R> {
    Continue(S),
    Done(R),
}

pub enum PortStatus<Ctx> {
    Idle,
    InProgress(Ctx),
}

pub trait SteppedProcess<T, S> {
    type Ctx;
    type Result;

    fn create_context(parent: &S, task: T, task_id: u8) -> Self::Ctx;
    fn step(parent: &mut S, ctx: &Self::Ctx) -> Step<Self::Ctx, Self::Result>;
}

pub struct Port<T, P: SteppedProcess<T, S>, S> {
    inbox: VecDeque<(T, u8)>,
    outbox: VecDeque<P::Result>,
    state: PortStatus<P::Ctx>,
}

impl<T, P: SteppedProcess<T, S>, S> Port<T, P, S> {
    pub fn new() -> Self {
        Self {
            inbox: VecDeque::new(),
            outbox: VecDeque::new(),
            state: PortStatus::Idle,
        }
    }

    pub fn send(&mut self, task: T, task_id: u8) {
        self.inbox.push_back((task, task_id));
    }

    pub fn pop(&mut self) -> Option<P::Result> {
        self.outbox.pop_front()
    }

    pub fn tick(&mut self, parent: &mut S) {
        if let PortStatus::Idle = self.state {
            if let Some(task) = self.inbox.pop_front() {
                self.state = PortStatus::InProgress(P::create_context(parent, task.0, task.1));
            }
        }

        let new_state = match &self.state {
            PortStatus::InProgress(ctx) => match P::step(parent, ctx) {
                Step::Continue(new_ctx) => PortStatus::InProgress(new_ctx),
                Step::Done(result) => {
                    self.outbox.push_back(result);
                    PortStatus::Idle
                }
            },
            PortStatus::Idle => PortStatus::Idle,
        };

        self.state = new_state;
    }
}
