//! Serial link to the Marlin board, run on a background thread.
//!
//! The UI talks to the worker through channels: it sends `Command`s and
//! receives `Event`s. The worker sends one line at a time and waits for `ok`
//! (like cyclone), numbers and checksums every line so Marlin can ask for a
//! resend, polls `M114` for the position while idle, and streams jobs.

pub mod protocol;
pub mod sim;
mod worker;

use std::io;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

pub use protocol::Position;

/// Name shown in the port list for the built-in simulated board.
pub const SIMULATOR: &str = "Simulator";

/// Anything bytes can be exchanged with: a real serial port or the simulator.
pub trait Link: Send {
    /// Read whatever is available; `Ok(0)` when nothing arrived before the timeout.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    fn write_all(&mut self, data: &[u8]) -> io::Result<()>;
}

struct RealPort(Box<dyn serialport::SerialPort>);

impl Link for RealPort {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.0.read(buf) {
            Err(e) if e.kind() == io::ErrorKind::TimedOut => Ok(0),
            other => other,
        }
    }
    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        io::Write::write_all(&mut self.0, data)?;
        io::Write::flush(&mut self.0)
    }
}

pub enum Command {
    /// Queue a G-code line typed by the user (or from a button). These go
    /// ahead of job lines, so they work during a job (e.g. M220).
    Send(String),
    /// Send M112 right away, ahead of anything queued.
    EmergencyStop,
    /// Stream these lines (comments and blank lines are skipped).
    StartJob(Arc<Vec<String>>),
    /// Stop feeding job lines and wait for the machine to come to rest.
    PauseJob,
    ResumeJob,
    /// Abandon the job and stop motion at once (M410).
    StopJob,
    Disconnect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogKind {
    /// Sent to the board.
    Tx,
    /// Received from the board.
    Rx,
    /// Received `ok` / busy keep-alive (hidden by the console filter).
    RxQuiet,
    /// Our own status messages.
    Info,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub enum JobEvent {
    Started { total: usize },
    /// `done` lines have been accepted by the board; `position` is where
    /// the last sent move is headed.
    Progress { done: usize, position: Position },
    /// Waiting for buffered moves to finish before calling it paused.
    Pausing,
    Paused,
    Resumed,
    /// Every line ran and the machine has stopped moving.
    Finished,
    Stopped,
    Failed(String),
}

#[derive(Clone, Debug)]
pub enum Event {
    /// Port opened; waiting for the board to answer.
    Opened,
    /// The board answered and line numbering is set up.
    Ready,
    Log(LogKind, String),
    Position(Position),
    /// Whether the user's commands are still being sent or executed.
    Busy(bool),
    /// Marlin halted (M112 or a fault) and needs a reset.
    Halted,
    /// The board has not answered for a while.
    NoReply,
    Job(JobEvent),
    /// The link closed, with an error message if it wasn't requested.
    Closed(Option<String>),
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Keep DTR and RTS low so opening the port doesn't reset an ESP32 board.
    pub hold_dtr_rts_low: bool,
}

/// UI-side handle to a running connection. Dropping it disconnects.
pub struct Connection {
    tx: Sender<Command>,
    rx: Receiver<Event>,
}

impl Connection {
    pub fn open(port: String, baud: u32, options: Options, ctx: eframe::egui::Context) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (evt_tx, evt_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("serial".into())
            .spawn(move || {
                let repaint = move || ctx.request_repaint();
                let link: Result<Box<dyn Link>, String> = if port == SIMULATOR {
                    Ok(Box::new(sim::SimMarlin::new()))
                } else {
                    open_port(&port, baud, options).map(|p| Box::new(p) as Box<dyn Link>)
                };
                match link {
                    Ok(link) => worker::run(link, cmd_rx, evt_tx, repaint),
                    Err(e) => {
                        let _ = evt_tx.send(Event::Closed(Some(e)));
                        repaint();
                    }
                }
            })
            .expect("spawn serial thread");
        Self {
            tx: cmd_tx,
            rx: evt_rx,
        }
    }

    fn command(&self, c: Command) {
        let _ = self.tx.send(c);
    }

    pub fn send(&self, line: impl Into<String>) {
        self.command(Command::Send(line.into()));
    }

    pub fn emergency_stop(&self) {
        self.command(Command::EmergencyStop);
    }

    pub fn start_job(&self, lines: Arc<Vec<String>>) {
        self.command(Command::StartJob(lines));
    }

    pub fn pause_job(&self) {
        self.command(Command::PauseJob);
    }

    pub fn resume_job(&self) {
        self.command(Command::ResumeJob);
    }

    pub fn stop_job(&self) {
        self.command(Command::StopJob);
    }

    pub fn events(&self) -> impl Iterator<Item = Event> + '_ {
        self.rx.try_iter()
    }

    /// Disconnect and wait (up to `timeout`) for the serial thread to finish,
    /// so anything it still has to send (like a quick stop) goes out first.
    pub fn shutdown(self, timeout: Duration) {
        let _ = self.tx.send(Command::Disconnect);
        let deadline = Instant::now() + timeout;
        while let Ok(e) = self.rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            if matches!(e, Event::Closed(_)) {
                break;
            }
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Disconnect);
    }
}

fn open_port(name: &str, baud: u32, options: Options) -> Result<RealPort, String> {
    let mut port = serialport::new(name, baud)
        .timeout(Duration::from_millis(10))
        .open()
        .map_err(|e| {
            let mut msg = format!("Could not open {name}: {e}");
            if cfg!(target_os = "linux")
                && e.kind() == serialport::ErrorKind::Io(io::ErrorKind::PermissionDenied)
            {
                msg += ". On Linux, add yourself to the serial group with \
                        `sudo usermod -aG dialout $USER`, then log out and back in.";
            }
            msg
        })?;
    if options.hold_dtr_rts_low {
        // The ESP32 auto-reset circuit pulls EN (reset) low only while RTS is
        // asserted and DTR is not. The port opens with both asserted (no
        // reset); releasing RTS first and then DTR never passes through that
        // combination, so connecting doesn't restart the board.
        let _ = port.write_request_to_send(false);
        let _ = port.write_data_terminal_ready(false);
    }
    Ok(RealPort(port))
}
