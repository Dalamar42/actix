use std::sync;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use actix::prelude::*;
use tokio::time::{delay_for, Duration, Instant};

#[derive(Debug)]
struct Num(usize);

impl Message for Num {
    type Result = ();
}

struct MyActor(Arc<AtomicUsize>, Arc<AtomicBool>, Running);

impl Actor for MyActor {
    type Context = actix::Context<Self>;

    fn stopping(&mut self, _: &mut Self::Context) -> Running {
        System::current().stop();
        Running::Stop
    }
}

impl StreamHandler<Num> for MyActor {
    fn handle(&mut self, msg: Num, _: &mut Context<MyActor>) {
        self.0.fetch_add(msg.0, Ordering::Relaxed);
    }

    fn finished(&mut self, _: &mut Context<MyActor>) {
        self.1.store(true, Ordering::Relaxed);
    }
}

#[actix_rt::test]
async fn test_stream() {
    let count = Arc::new(AtomicUsize::new(0));
    let err = Arc::new(AtomicBool::new(false));
    let items = vec![Num(1), Num(1), Num(1), Num(1), Num(1), Num(1), Num(1)];

    let act_count = Arc::clone(&count);
    let act_err = Arc::clone(&err);

    MyActor::create(move |ctx| {
        MyActor::add_stream(futures::stream::iter::<_>(items), ctx);
        MyActor(act_count, act_err, Running::Stop)
    });

    delay_for(Duration::new(0, 1_000_000)).await;

    assert_eq!(count.load(Ordering::Relaxed), 7);
    assert!(err.load(Ordering::Relaxed));
}

struct MySyncActor {
    started: Arc<AtomicUsize>,
    stopping: Arc<AtomicUsize>,
    stopped: Arc<AtomicUsize>,
    msgs: Arc<AtomicUsize>,
    stop: bool,
}

impl Actor for MySyncActor {
    type Context = actix::SyncContext<Self>;

    fn started(&mut self, _: &mut Self::Context) {
        self.started.fetch_add(1, Ordering::Relaxed);
    }
    fn stopping(&mut self, _: &mut Self::Context) -> Running {
        self.stopping.fetch_add(1, Ordering::Relaxed);
        Running::Continue
    }
    fn stopped(&mut self, _: &mut Self::Context) {
        self.stopped.fetch_add(1, Ordering::Relaxed);
    }
}

impl actix::Handler<Num> for MySyncActor {
    type Result = ();

    fn handle(&mut self, msg: Num, ctx: &mut Self::Context) {
        self.msgs.fetch_add(msg.0, Ordering::Relaxed);
        if self.stop {
            ctx.stop();
        }
    }
}

#[test]
fn test_restart_sync_actor() {
    let started = Arc::new(AtomicUsize::new(0));
    let stopping = Arc::new(AtomicUsize::new(0));
    let stopped = Arc::new(AtomicUsize::new(0));
    let msgs = Arc::new(AtomicUsize::new(0));

    let started1 = Arc::clone(&started);
    let stopping1 = Arc::clone(&stopping);
    let stopped1 = Arc::clone(&stopped);
    let msgs1 = Arc::clone(&msgs);

    System::run(move || {
        let addr = SyncArbiter::start(1, move || MySyncActor {
            started: Arc::clone(&started1),
            stopping: Arc::clone(&stopping1),
            stopped: Arc::clone(&stopped1),
            msgs: Arc::clone(&msgs1),
            stop: started1.load(Ordering::Relaxed) == 0,
        });

        addr.do_send(Num(2));

        actix_rt::spawn(async move {
            let _ = addr.send(Num(4)).await;
            delay_for(Duration::new(0, 1_000_000)).await;
            System::current().stop();
        });
    })
    .unwrap();

    assert_eq!(started.load(Ordering::Relaxed), 2);
    assert_eq!(stopping.load(Ordering::Relaxed), 2);
    assert_eq!(stopped.load(Ordering::Relaxed), 2);
    assert_eq!(msgs.load(Ordering::Relaxed), 6);
}

struct IntervalActor {
    elapses_left: usize,
    sender: sync::mpsc::Sender<Instant>,
    instant: Option<Instant>,
}

impl IntervalActor {
    pub fn new(elapses_left: usize, sender: sync::mpsc::Sender<Instant>) -> Self {
        Self {
            //We stop at 0, so add 1 to make number of intervals equal to elapses_left
            elapses_left: elapses_left + 1,
            sender,
            instant: None,
        }
    }
}

impl Actor for IntervalActor {
    type Context = actix::Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.instant = Some(Instant::now());
        ctx.run_interval(Duration::from_millis(110), move |act, ctx| {
            act.elapses_left -= 1;

            if act.elapses_left == 0 {
                act.sender
                    .send(act.instant.take().expect("To have Instant"))
                    .expect("To send result");
                ctx.stop();
                System::current().stop();
            }
        });
    }
}

#[test]
fn test_run_interval() {
    const MAX_WAIT: Duration = Duration::from_millis(10_000);

    let (sender, receiver) = sync::mpsc::channel();
    std::thread::spawn(move || {
        System::run(move || {
            let _addr = IntervalActor::new(10, sender).start();
        })
        .unwrap();
    });

    let result = receiver
        .recv_timeout(MAX_WAIT)
        .expect("To receive response in time");

    //We wait 10 intervals by ~100ms
    assert_eq!(result.elapsed().as_secs(), 1);
}
