//! Wayland thread: connects to wl_display and subscribes to ext_idle_notify_v1
//! so we can answer GetIdletime queries. Screenshots are handled elsewhere
//! (via xdg-desktop-portal D-Bus), so the Wayland-side of this bridge is
//! intentionally tiny.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use calloop::EventLoop;
use calloop::ping::{Ping, PingSource};
use calloop_wayland_source::WaylandSource;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::{
    self, ExtIdleNotificationV1,
};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1;

/// Idle notifier polling resolution. Each `resumed` event refreshes `last_active`,
/// so a smaller timeout = better resolution at negligible cost. The Python original
/// uses 1000ms; we match.
const IDLE_TIMEOUT_MS: u32 = 1000;

pub struct Handles {
    pub last_active: Arc<Mutex<Instant>>,
    pub shutdown: Ping,
    pub thread: JoinHandle<Result<()>>,
}

pub fn start() -> Result<Handles> {
    let (shutdown_ping, shutdown_source) = calloop::ping::make_ping()?;
    let last_active = Arc::new(Mutex::new(Instant::now()));
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);

    let la = last_active.clone();
    let thread = thread::Builder::new()
        .name("wayland".into())
        .spawn(move || {
            let res = run_thread(shutdown_source, la, ready_tx.clone());
            if let Err(e) = &res {
                let _ = ready_tx.send(Err(anyhow!("wayland thread failed: {e:#}")));
            }
            res
        })?;

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(Handles {
            last_active,
            shutdown: shutdown_ping,
            thread,
        }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(anyhow!("wayland thread exited before signaling ready")),
    }
}

fn run_thread(
    shutdown_source: PingSource,
    last_active: Arc<Mutex<Instant>>,
    ready_tx: std::sync::mpsc::SyncSender<Result<()>>,
) -> Result<()> {
    let conn = Connection::connect_to_env().context("connecting to wl_display")?;
    let (globals, event_queue) =
        registry_queue_init::<State>(&conn).context("initializing wl_registry")?;
    let qh = event_queue.handle();

    let idle_notifier: ExtIdleNotifierV1 = globals
        .bind(&qh, 1..=2, ())
        .context("binding ext_idle_notifier_v1 (compositor must support ext-idle-notify)")?;
    let seat: WlSeat = globals.bind(&qh, 1..=10, ()).context("binding wl_seat")?;

    let idle_notification =
        idle_notifier.get_idle_notification(IDLE_TIMEOUT_MS, &seat, &qh, ());
    log::info!(
        "wayland: bound ext_idle_notifier_v1; subscribed with {} ms timeout",
        IDLE_TIMEOUT_MS
    );

    let mut state = State {
        last_active,
        _idle_notification: idle_notification,
        _seat: seat,
        _idle_notifier: idle_notifier,
    };

    let mut event_loop: EventLoop<State> = EventLoop::try_new()?;
    let handle = event_loop.handle();

    WaylandSource::new(conn, event_queue)
        .insert(handle.clone())
        .map_err(|e| anyhow!("inserting WaylandSource into calloop: {e}"))?;

    let stop = Rc::new(RefCell::new(false));
    let stop_for_source = stop.clone();
    handle
        .insert_source(shutdown_source, move |_, _, _| {
            *stop_for_source.borrow_mut() = true;
        })
        .map_err(|e| anyhow!("inserting shutdown source: {e}"))?;

    let _ = ready_tx.send(Ok(()));

    while !*stop.borrow() {
        event_loop.dispatch(None, &mut state).context("calloop dispatch")?;
    }
    log::info!("wayland thread exiting");
    Ok(())
}

// --- State -----------------------------------------------------------------

struct State {
    last_active: Arc<Mutex<Instant>>,
    _idle_notification: ExtIdleNotificationV1,
    _seat: WlSeat,
    _idle_notifier: ExtIdleNotifierV1,
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WlRegistry,
        _event: <WlRegistry as Proxy>::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

macro_rules! noop_dispatch {
    ($ty:ty) => {
        impl Dispatch<$ty, ()> for State {
            fn event(
                _state: &mut Self,
                _proxy: &$ty,
                _event: <$ty as Proxy>::Event,
                _data: &(),
                _conn: &Connection,
                _qh: &QueueHandle<Self>,
            ) {
            }
        }
    };
}

noop_dispatch!(WlSeat);
noop_dispatch!(ExtIdleNotifierV1);

impl Dispatch<ExtIdleNotificationV1, ()> for State {
    fn event(
        state: &mut Self,
        _proxy: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => {
                log::debug!("idle: idled");
            }
            ext_idle_notification_v1::Event::Resumed => {
                log::debug!("idle: resumed");
                *state.last_active.lock().unwrap() = Instant::now();
            }
            _ => {}
        }
    }
}
