//! The synthesis thread: runs engine calls in order, off the control path.
//! The worker queues jobs, moves the one a play waits for to the front, and
//! drops the jobs of an item that ended (cancelling it if in flight).
use sonara_core::reader::ItemId;
use sonara_engine::{Engine, EngineId, PcmChunk};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

/// One chunk to synthesize, with the settings it was requested under.
pub(crate) struct Job {
    pub item: ItemId,
    pub chunk: usize,
    pub text: String,
    pub voice: String,
    pub rate: u32,
    pub engine: Arc<dyn Engine>,
}

enum Task {
    /// Prepare an engine; a failure is only logged.
    Warm(Arc<dyn Engine>),
    Chunk(Job),
}

/// What the thread hands back.
pub(crate) enum Done {
    Chunk {
        item: ItemId,
        chunk: usize,
        result: Result<Vec<PcmChunk>, sonara_engine::Error>,
    },
    WarmFailed {
        engine: EngineId,
        message: String,
    },
}

#[derive(Default)]
struct Queue {
    tasks: VecDeque<Task>,
    /// The task running and its engine, for cancel: the chunk's item, or
    /// `None` for a warm-up (cancelled only on shutdown).
    in_flight: Option<(Option<ItemId>, Arc<dyn Engine>)>,
    closed: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub(crate) struct Synth {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Synth {
    /// Start the thread; `done` is called on it for every finished task.
    pub fn start(done: impl Fn(Done) + Send + 'static) -> std::io::Result<Synth> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue::default()),
            wake: Condvar::new(),
        });
        let s = shared.clone();
        let thread = thread::Builder::new()
            .name("sonara-synth".into())
            .spawn(move || run(&s, done))?;
        Ok(Synth {
            shared,
            thread: Some(thread),
        })
    }

    pub fn warm(&self, engine: Arc<dyn Engine>) {
        self.push(Task::Warm(engine));
    }

    pub fn push_job(&self, job: Job) {
        self.push(Task::Chunk(job));
    }

    fn push(&self, task: Task) {
        self.shared.lock().tasks.push_back(task);
        self.shared.wake.notify_one();
    }

    /// Move the queued job for this chunk to the front: a play waits for it.
    pub fn promote(&self, item: ItemId, chunk: usize) {
        let mut q = self.shared.lock();
        let at = q
            .tasks
            .iter()
            .position(|t| matches!(t, Task::Chunk(j) if j.item == item && j.chunk == chunk));
        if let Some(task) = at.and_then(|i| q.tasks.remove(i)) {
            q.tasks.push_front(task);
        }
    }

    /// The item ended: forget its queued jobs and cancel it if in flight.
    pub fn drop_item(&self, item: ItemId) {
        let mut q = self.shared.lock();
        q.tasks
            .retain(|t| !matches!(t, Task::Chunk(j) if j.item == item));
        if let Some((busy, engine)) = &q.in_flight {
            if *busy == Some(item) {
                engine.cancel();
            }
        }
    }

    /// Cancel everything and join the thread. The engine contract makes a
    /// cancelled synthesis end promptly.
    pub fn shutdown(&mut self) {
        {
            let mut q = self.shared.lock();
            q.closed = true;
            q.tasks.clear();
            if let Some((_, engine)) = &q.in_flight {
                engine.cancel();
            }
        }
        self.shared.wake.notify_one();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Synth {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run(shared: &Shared, done: impl Fn(Done)) {
    loop {
        let task = {
            let mut q = shared.lock();
            loop {
                if q.closed {
                    return;
                }
                if let Some(task) = q.tasks.pop_front() {
                    q.in_flight = Some(match &task {
                        Task::Warm(engine) => (None, engine.clone()),
                        Task::Chunk(job) => {
                            // Under the lock `drop_item` takes: its cancel
                            // ends this chunk even before `synthesize` runs.
                            job.engine.begin();
                            (Some(job.item), job.engine.clone())
                        }
                    });
                    break task;
                }
                q = shared.wake.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        match task {
            Task::Warm(engine) => {
                let result = engine.warm();
                shared.lock().in_flight = None;
                if let Err(e) = result {
                    done(Done::WarmFailed {
                        engine: engine.id(),
                        message: format!("engine '{}' is not ready: {e}", engine.id()),
                    });
                }
            }
            Task::Chunk(job) => {
                let result = job
                    .engine
                    .synthesize(&job.text, &job.voice, job.rate)
                    .and_then(|stream| stream.collect::<Result<Vec<_>, _>>());
                shared.lock().in_flight = None;
                done(Done::Chunk {
                    item: job.item,
                    chunk: job.chunk,
                    result,
                });
            }
        }
    }
}
