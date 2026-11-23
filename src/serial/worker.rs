//! The serial thread: send one line, wait for `ok`, repeat.
//!
//! Priority when the board is ready for the next line: lines Marlin asked to
//! have resent, then queued commands (user commands, pause/stop helpers,
//! position polls), then the next job line.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use super::protocol::{self, Position, Response};
use super::{Command, Event, JobEvent, Link, LogKind};

/// How long to wait for the board's boot banner before talking to it anyway.
const BOOT_WAIT: Duration = Duration::from_millis(2500);
/// Position poll interval while idle.
const POLL_EVERY: Duration = Duration::from_millis(1000);
/// Warn if the board has said nothing for this long while we wait for `ok`.
const SILENCE_WARNING: Duration = Duration::from_secs(10);
/// Give up after this many resend requests in a row.
const MAX_RESENDS: u32 = 20;
/// Sent lines kept for resends.
const HISTORY: usize = 256;
/// Job progress events at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tag {
    /// Typed by the user or from a button: shown in the console.
    Manual,
    /// Background M114: its replies are not logged.
    Poll,
    /// Connection setup (M110, M115).
    Init,
    /// Job line at this index.
    Job(usize),
    /// M400 used to know when the machine has stopped (pause / end of job).
    Sync,
    /// M410 quick stop after the user stops a job.
    Quick,
}

struct InFlight {
    /// Line number, or `None` for unnumbered lines (M110).
    n: Option<u32>,
    tag: Tag,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Running,
    /// M400 queued; paused once it is acknowledged.
    Pausing,
    Paused,
    /// Every line sent; waiting for the final M400.
    Finishing,
}

struct Job {
    lines: Arc<Vec<String>>,
    /// Next line index to look at.
    next: usize,
    /// Lines before this index have been accepted by the board.
    done: usize,
    phase: Phase,
    /// Target of the last move sent.
    position: Position,
    last_progress: Instant,
}

struct Worker<R: Fn()> {
    link: Box<dyn Link>,
    events: Sender<Event>,
    repaint: R,
    queue: VecDeque<(String, Tag)>,
    in_flight: Option<InFlight>,
    next_line: u32,
    history: VecDeque<(u32, String, Tag)>,
    /// Lines Marlin asked us to send again, before anything else.
    resend: VecDeque<u32>,
    resend_requested: bool,
    resends_in_a_row: u32,
    rx_buf: Vec<u8>,
    ready: bool,
    opened_at: Instant,
    last_poll: Instant,
    last_rx: Instant,
    warned_silence: bool,
    busy: bool,
    job: Option<Job>,
}

pub fn run(link: Box<dyn Link>, commands: Receiver<Command>, events: Sender<Event>, repaint: impl Fn()) {
    let now = Instant::now();
    let mut w = Worker {
        link,
        events,
        repaint,
        queue: VecDeque::new(),
        in_flight: None,
        next_line: 1,
        history: VecDeque::new(),
        resend: VecDeque::new(),
        resend_requested: false,
        resends_in_a_row: 0,
        rx_buf: Vec::new(),
        ready: false,
        opened_at: now,
        last_poll: now,
        last_rx: now,
        warned_silence: false,
        busy: false,
        job: None,
    };
    w.emit(Event::Opened);
    w.log(LogKind::Info, "Port open, waiting for the board…");
    let error = w.run_loop(&commands);
    if w.job.is_some() {
        let why = error.clone().unwrap_or_else(|| "Disconnected during the job.".into());
        w.end_job(JobEvent::Failed(why));
    }
    keep_awake(false);
    w.emit(Event::Closed(error));
}

impl<R: Fn()> Worker<R> {
    fn emit(&self, e: Event) {
        let _ = self.events.send(e);
        (self.repaint)();
    }

    fn log(&self, kind: LogKind, text: impl Into<String>) {
        self.emit(Event::Log(kind, text.into()));
    }

    /// Returns `None` on a requested disconnect, or the error that ended the link.
    fn run_loop(&mut self, commands: &Receiver<Command>) -> Option<String> {
        let mut buf = [0u8; 1024];
        loop {
            loop {
                match commands.try_recv() {
                    Ok(Command::Disconnect) | Err(TryRecvError::Disconnected) => {
                        // Don't drop a pending quick stop on the floor.
                        if self.queue.iter().any(|(_, t)| *t == Tag::Quick) {
                            let _ = self.write("M410");
                        }
                        self.log(LogKind::Info, "Disconnected.");
                        return None;
                    }
                    Ok(cmd) => {
                        if let Err(e) = self.command(cmd) {
                            return Some(e);
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }

            match self.link.read(&mut buf) {
                Ok(0) => {}
                Ok(n) => {
                    self.last_rx = Instant::now();
                    self.warned_silence = false;
                    self.rx_buf.extend_from_slice(&buf[..n]);
                    while let Some(i) = self.rx_buf.iter().position(|b| *b == b'\n') {
                        let raw: Vec<u8> = self.rx_buf.drain(..=i).collect();
                        let line = String::from_utf8_lossy(&raw).trim().to_string();
                        if !line.is_empty()
                            && let Err(e) = self.handle_line(&line) {
                                return Some(e);
                            }
                    }
                }
                Err(e) => {
                    let msg = format!("Connection lost: {e}");
                    self.log(LogKind::Error, msg.clone());
                    return Some(msg);
                }
            }

            if !self.ready && self.in_flight.is_none() && self.opened_at.elapsed() >= BOOT_WAIT {
                // No banner (the board was already running): start anyway.
                self.begin_session();
            }
            let job_idle = self.job.as_ref().is_none_or(|j| j.phase == Phase::Paused);
            if self.ready
                && job_idle
                && self.in_flight.is_none()
                && self.queue.is_empty()
                && self.last_poll.elapsed() >= POLL_EVERY
            {
                self.queue.push_back(("M114".into(), Tag::Poll));
            }
            if let Err(e) = self.pump() {
                return Some(e);
            }
            if self.in_flight.is_some()
                && !self.warned_silence
                && self.last_rx.elapsed() >= SILENCE_WARNING
            {
                self.warned_silence = true;
                self.log(
                    LogKind::Error,
                    "No reply from the board for 10 s; still waiting for \"ok\".",
                );
                self.emit(Event::NoReply);
            }
            self.update_busy();
            self.report_progress(false);
        }
    }

    fn command(&mut self, cmd: Command) -> Result<(), String> {
        match cmd {
            Command::Send(line) => {
                let cmd = protocol::clean_command(&line).to_string();
                if !cmd.is_empty() {
                    self.queue.push_back((cmd, Tag::Manual));
                }
            }
            Command::EmergencyStop => {
                if self.job.is_some() {
                    self.end_job(JobEvent::Failed("Emergency stop.".into()));
                }
                self.queue.clear();
                self.resend.clear();
                self.in_flight = None;
                self.log(LogKind::Tx, "M112 (emergency stop)");
                self.write("M112")?;
            }
            Command::StartJob(lines) => {
                if self.job.is_some() {
                    self.log(LogKind::Error, "A job is already running.");
                } else if !self.ready {
                    self.emit(Event::Job(JobEvent::Failed("The board is not ready yet.".into())));
                } else {
                    let total = lines.len();
                    self.job = Some(Job {
                        lines,
                        next: 0,
                        done: 0,
                        phase: Phase::Running,
                        position: Position::default(),
                        last_progress: Instant::now(),
                    });
                    keep_awake(true);
                    self.emit(Event::Job(JobEvent::Started { total }));
                }
            }
            Command::PauseJob => {
                if let Some(job) = &mut self.job
                    && job.phase == Phase::Running {
                        job.phase = Phase::Pausing;
                        self.queue.push_back(("M400".into(), Tag::Sync));
                        self.emit(Event::Job(JobEvent::Pausing));
                    }
            }
            Command::ResumeJob => {
                if let Some(job) = &mut self.job
                    && matches!(job.phase, Phase::Paused | Phase::Pausing) {
                        job.phase = Phase::Running;
                        self.queue.retain(|(_, t)| *t != Tag::Sync);
                        self.emit(Event::Job(JobEvent::Resumed));
                    }
            }
            Command::StopJob => {
                if self.job.is_some() {
                    self.queue.retain(|(_, t)| *t != Tag::Sync);
                    // Ahead of everything else: discard Marlin's buffered moves.
                    self.queue.push_front(("M410".into(), Tag::Quick));
                    self.log(LogKind::Tx, "M410 (quick stop)");
                    self.end_job(JobEvent::Stopped);
                }
            }
            Command::Disconnect => unreachable!("handled by the caller"),
        }
        Ok(())
    }

    fn end_job(&mut self, event: JobEvent) {
        self.report_progress(true);
        self.job = None;
        keep_awake(false);
        self.emit(Event::Job(event));
        self.last_poll = Instant::now() - POLL_EVERY;
    }

    fn report_progress(&mut self, force: bool) {
        let Some(job) = &mut self.job else { return };
        if force || job.last_progress.elapsed() >= PROGRESS_EVERY {
            job.last_progress = Instant::now();
            let e = JobEvent::Progress {
                done: job.done,
                position: job.position,
            };
            self.emit(Event::Job(e));
        }
    }

    /// Reset line numbering and ask who we are talking to.
    fn begin_session(&mut self) {
        self.queue.retain(|(_, tag)| *tag == Tag::Manual);
        self.in_flight = None;
        self.resend.clear();
        self.resend_requested = false;
        self.history.clear();
        self.queue.push_front(("M115".into(), Tag::Init));
        self.queue.push_front(("M110 N0".into(), Tag::Init));
        self.ready = true;
        self.last_poll = Instant::now() - POLL_EVERY;
    }

    fn handle_line(&mut self, line: &str) -> Result<(), String> {
        let tag = self.in_flight.as_ref().map(|f| f.tag);
        let response = protocol::parse_response(line);
        match &response {
            Response::Ok => {
                if tag == Some(Tag::Manual) {
                    self.log(LogKind::RxQuiet, line);
                }
                self.on_ok()?;
            }
            Response::Resend(n) => {
                self.log(LogKind::Info, format!("Board asked to resend line {n}"));
                self.resends_in_a_row += 1;
                if self.resends_in_a_row > MAX_RESENDS {
                    let msg = "Too many resend requests in a row; check the cable and baud rate.";
                    self.log(LogKind::Error, msg);
                    return Err(msg.into());
                }
                let last_sent = self.next_line.saturating_sub(1);
                self.resend = (*n..=last_sent).collect();
                self.resend_requested = true;
            }
            Response::Error(text) => {
                // Checksum/line-number errors are followed by a Resend; just note them.
                let kind = if text.contains("checksum") || text.contains("Line Number") {
                    LogKind::Info
                } else {
                    LogKind::Error
                };
                self.log(kind, format!("Error:{text}"));
                if text.contains("kill()") || text.contains("halted") {
                    self.queue.clear();
                    self.in_flight = None;
                    if self.job.is_some() {
                        self.end_job(JobEvent::Failed("The board halted.".into()));
                    }
                    self.log(
                        LogKind::Error,
                        "The board halted. Reset or power-cycle it, then reconnect.",
                    );
                    self.emit(Event::Halted);
                }
            }
            Response::Busy => {
                if tag == Some(Tag::Manual) {
                    self.log(LogKind::RxQuiet, line);
                }
            }
            Response::Start => {
                self.log(LogKind::Rx, line);
                if self.ready {
                    self.log(LogKind::Info, "The board restarted.");
                }
                if self.job.is_some() {
                    self.end_job(JobEvent::Failed("The board restarted during the job.".into()));
                }
                // Ready is announced once the new M110 is acknowledged.
                self.begin_session();
            }
            Response::Position(p) => {
                self.emit(Event::Position(*p));
                if tag != Some(Tag::Poll) {
                    self.log(LogKind::Rx, line);
                }
            }
            Response::Other => {
                if tag != Some(Tag::Poll) {
                    self.log(LogKind::Rx, line);
                }
            }
        }
        Ok(())
    }

    fn on_ok(&mut self) -> Result<(), String> {
        let Some(done) = self.in_flight.take() else { return Ok(()) };
        if self.resend_requested {
            // This ok belongs to the rejected line; resend instead of moving on.
            self.resend_requested = false;
            return self.send_resend();
        }
        self.resends_in_a_row = 0;
        match done.tag {
            Tag::Init if done.n.is_none() => {
                // M110 N0 accepted: numbering restarts at 1.
                self.next_line = 1;
                self.log(LogKind::Info, "Connected.");
                self.emit(Event::Ready);
            }
            Tag::Poll => self.last_poll = Instant::now(),
            Tag::Manual => {
                // Refresh the position soon after a manual command finishes.
                self.last_poll = Instant::now() - POLL_EVERY + Duration::from_millis(150);
            }
            Tag::Job(i) => {
                if let Some(job) = &mut self.job {
                    job.done = job.done.max(i + 1);
                }
            }
            Tag::Sync => match self.job.as_ref().map(|j| j.phase) {
                Some(Phase::Pausing) => {
                    if let Some(job) = &mut self.job {
                        job.phase = Phase::Paused;
                    }
                    self.report_progress(true);
                    self.emit(Event::Job(JobEvent::Paused));
                    self.last_poll = Instant::now() - POLL_EVERY;
                }
                Some(Phase::Finishing) => {
                    if let Some(job) = &mut self.job {
                        job.done = job.lines.len();
                    }
                    self.end_job(JobEvent::Finished);
                }
                _ => {}
            },
            Tag::Init | Tag::Quick => {}
        }
        Ok(())
    }

    fn send_resend(&mut self) -> Result<(), String> {
        let Some(n) = self.resend.pop_front() else {
            self.in_flight = None;
            return Ok(());
        };
        let Some((line, tag)) = self
            .history
            .iter()
            .find(|(h, _, _)| *h == n)
            .map(|(_, l, t)| (l.clone(), *t))
        else {
            let msg = format!("The board asked for line {n}, which is no longer available.");
            self.log(LogKind::Error, msg.clone());
            return Err(msg);
        };
        self.in_flight = Some(InFlight { n: Some(n), tag });
        self.write(&protocol::numbered(n, &line))
    }

    /// Send the next line if nothing is waiting for `ok`.
    fn pump(&mut self) -> Result<(), String> {
        if self.in_flight.is_some() || !self.ready {
            return Ok(());
        }
        if !self.resend.is_empty() {
            return self.send_resend();
        }
        if let Some((cmd, tag)) = self.queue.pop_front() {
            return self.send_line(cmd, tag);
        }
        let mut next = None;
        let mut finished = false;
        if let Some(job) = &mut self.job
            && job.phase == Phase::Running {
                while job.next < job.lines.len() {
                    let i = job.next;
                    job.next += 1;
                    let cmd = protocol::clean_command(&job.lines[i]);
                    if !cmd.is_empty() {
                        track_position(&mut job.position, cmd);
                        next = Some((cmd.to_string(), Tag::Job(i)));
                        break;
                    }
                }
                if next.is_none() {
                    // Everything sent: wait for the last moves to finish.
                    job.phase = Phase::Finishing;
                    finished = true;
                }
            }
        if finished {
            self.queue.push_back(("M400".into(), Tag::Sync));
        }
        match next {
            Some((cmd, tag)) => self.send_line(cmd, tag),
            None => Ok(()),
        }
    }

    fn send_line(&mut self, cmd: String, tag: Tag) -> Result<(), String> {
        if tag == Tag::Manual {
            self.log(LogKind::Tx, cmd.clone());
        }
        if cmd.starts_with("M110") {
            self.in_flight = Some(InFlight { n: None, tag });
            return self.write(&cmd);
        }
        let n = self.next_line;
        self.next_line += 1;
        let line = protocol::numbered(n, &cmd);
        self.history.push_back((n, cmd, tag));
        if self.history.len() > HISTORY {
            self.history.pop_front();
        }
        self.in_flight = Some(InFlight { n: Some(n), tag });
        self.write(&line)
    }

    fn write(&mut self, line: &str) -> Result<(), String> {
        let mut bytes = line.as_bytes().to_vec();
        bytes.push(b'\n');
        self.link.write_all(&bytes).map_err(|e| {
            let msg = format!("Write failed: {e}");
            self.log(LogKind::Error, msg.clone());
            msg
        })
    }

    fn update_busy(&mut self) {
        let busy = self.queue.iter().any(|(_, t)| *t == Tag::Manual)
            || self.in_flight.as_ref().is_some_and(|f| f.tag == Tag::Manual);
        if busy != self.busy {
            self.busy = busy;
            self.emit(Event::Busy(busy));
        }
    }
}

/// Follow where absolute G0/G1/G92 lines put the machine.
fn track_position(pos: &mut Position, cmd: &str) {
    let mut words = cmd.split_whitespace();
    if !matches!(words.next(), Some("G0" | "G1" | "G00" | "G01" | "G92")) {
        return;
    }
    for w in words {
        let (axis, value) = w.split_at(1);
        let Ok(v) = value.parse::<f64>() else { continue };
        match axis {
            "X" => pos.x = v,
            "Y" => pos.y = v,
            "Z" => pos.z = v,
            _ => {}
        }
    }
}

/// Keep Windows from sleeping while a job runs (sleep would cut the USB link).
/// The setting belongs to this thread and is dropped when it exits.
#[cfg(windows)]
fn keep_awake(on: bool) {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetThreadExecutionState(flags: u32) -> u32;
    }
    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
    let flags = if on {
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED
    } else {
        ES_CONTINUOUS
    };
    // SAFETY: plain Win32 call with documented flag values and no pointers.
    unsafe {
        SetThreadExecutionState(flags);
    }
}

/// On Linux, hold a logind inhibitor (via `systemd-inhibit`) for the job.
#[cfg(target_os = "linux")]
fn keep_awake(on: bool) {
    use std::process::{Child, Command, Stdio};
    use std::sync::Mutex;
    static INHIBITOR: Mutex<Option<Child>> = Mutex::new(None);
    let Ok(mut held) = INHIBITOR.lock() else { return };
    if let Some(mut child) = held.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    if on {
        *held = Command::new("systemd-inhibit")
            .args([
                "--what=sleep:idle",
                "--who=Typhoon",
                "--why=Winding a part",
                "--mode=block",
                "sleep",
                "infinity",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok();
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn keep_awake(_on: bool) {}

#[cfg(test)]
mod tests {
    use super::super::sim::SimMarlin;
    use super::*;
    use std::sync::Mutex;
    use std::sync::mpsc;

    struct Harness {
        cmd: mpsc::Sender<Command>,
        evt: mpsc::Receiver<Event>,
        executed: Arc<Mutex<Vec<String>>>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl Harness {
        fn new(faults_every: u32) -> Self {
            let sim = SimMarlin::new().with_faults(faults_every);
            let executed = sim.executed.clone();
            let (cmd, cmd_rx) = mpsc::channel();
            let (evt_tx, evt) = mpsc::channel();
            let handle = std::thread::spawn(move || run(Box::new(sim), cmd_rx, evt_tx, || {}));
            Self {
                cmd,
                evt,
                executed,
                handle: Some(handle),
            }
        }

        /// Wait for an event matching `pred`, failing after 20 s.
        fn wait_for(&self, what: &str, mut pred: impl FnMut(&Event) -> bool) -> Event {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                if let Ok(e) = self.evt.recv_timeout(Duration::from_millis(50)) {
                    if let Event::Closed(err) = &e {
                        panic!("closed while waiting for {what}: {err:?}");
                    }
                    if pred(&e) {
                        return e;
                    }
                }
            }
        }

        fn executed_moves(&self) -> Vec<String> {
            self.executed
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.starts_with('G'))
                .cloned()
                .collect()
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = self.cmd.send(Command::Disconnect);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }

    fn fixture(text: &str) -> Arc<Vec<String>> {
        Arc::new(text.replace("\r\n", "\n").lines().map(str::to_owned).collect())
    }

    fn moves(lines: &[String]) -> Vec<String> {
        lines
            .iter()
            .map(|l| protocol::clean_command(l).to_string())
            .filter(|c| c.starts_with('G'))
            .collect()
    }

    /// Drive the worker against the simulator with injected checksum errors:
    /// every command must reach the board exactly once, in order.
    #[test]
    fn resends_deliver_every_line_once_in_order() {
        let h = Harness::new(7);
        let sent: Vec<String> = (0..120).map(|i| format!("G0 X{i} Y{}", i * 3)).collect();
        for s in &sent {
            h.cmd.send(Command::Send(s.clone())).unwrap();
        }
        h.cmd.send(Command::Send("M114".into())).unwrap();

        let mut saw_busy = false;
        let mut resends = 0;
        let mut position = None;
        h.wait_for("commands to finish", |e| match e {
            Event::Busy(true) => {
                saw_busy = true;
                false
            }
            Event::Busy(false) => saw_busy,
            Event::Position(p) => {
                position = Some(*p);
                false
            }
            Event::Log(LogKind::Info, t) if t.starts_with("Board asked to resend") => {
                resends += 1;
                false
            }
            _ => false,
        });
        assert!(resends > 10, "expected injected resends, saw {resends}");
        assert_eq!(h.executed_moves(), sent);
        let p = position.expect("position reported");
        assert_eq!((p.x, p.y), (119.0, 357.0));
    }

    #[test]
    fn job_streams_every_line_once_and_finishes() {
        let h = Harness::new(11);
        h.wait_for("ready", |e| matches!(e, Event::Ready));
        let job = fixture(include_str!("../../tests/fixtures/hoop.gcode"));
        h.cmd.send(Command::StartJob(job.clone())).unwrap();
        h.wait_for("start", |e| matches!(e, Event::Job(JobEvent::Started { .. })));
        let mut last_done = 0;
        h.wait_for("finish", |e| match e {
            Event::Job(JobEvent::Progress { done, .. }) => {
                assert!(*done >= last_done, "progress went backwards");
                last_done = *done;
                false
            }
            Event::Job(JobEvent::Finished) => true,
            Event::Job(other @ (JobEvent::Failed(_) | JobEvent::Stopped)) => panic!("{other:?}"),
            _ => false,
        });
        assert_eq!(last_done, job.len());
        assert_eq!(h.executed_moves(), moves(&job));
    }

    #[test]
    fn pause_holds_and_resume_completes() {
        let h = Harness::new(13);
        h.wait_for("ready", |e| matches!(e, Event::Ready));
        let job = fixture(include_str!("../../tests/fixtures/hoop.gcode"));
        h.cmd.send(Command::StartJob(job.clone())).unwrap();
        h.wait_for("some progress", |e| {
            matches!(e, Event::Job(JobEvent::Progress { done, .. }) if *done > 40)
        });
        h.cmd.send(Command::PauseJob).unwrap();
        h.wait_for("paused", |e| matches!(e, Event::Job(JobEvent::Paused)));
        let at_pause = h.executed_moves().len();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(h.executed_moves().len(), at_pause, "moves kept coming while paused");
        assert!(at_pause < moves(&job).len());

        // A command typed while paused goes through.
        h.cmd.send(Command::Send("M220 S80".into())).unwrap();
        h.cmd.send(Command::ResumeJob).unwrap();
        h.wait_for("finish", |e| matches!(e, Event::Job(JobEvent::Finished)));
        assert_eq!(h.executed_moves(), moves(&job));
        assert!(h.executed.lock().unwrap().iter().any(|c| c == "M220 S80"));
    }

    /// Quitting mid-job sends StopJob and Disconnect back to back; the quick
    /// stop must still reach the board before the thread exits.
    #[test]
    fn quit_mid_job_still_sends_quick_stop() {
        let mut h = Harness::new(1000);
        h.wait_for("ready", |e| matches!(e, Event::Ready));
        let job = fixture(include_str!("../../tests/fixtures/helical.gcode"));
        h.cmd.send(Command::StartJob(job)).unwrap();
        h.wait_for("some progress", |e| {
            matches!(e, Event::Job(JobEvent::Progress { done, .. }) if *done > 50)
        });
        h.cmd.send(Command::StopJob).unwrap();
        h.cmd.send(Command::Disconnect).unwrap();
        h.handle.take().unwrap().join().unwrap();
        assert!(h.executed.lock().unwrap().iter().any(|c| c == "M410"));
    }

    #[test]
    fn stop_sends_quick_stop_and_ends_job() {
        let h = Harness::new(1000);
        h.wait_for("ready", |e| matches!(e, Event::Ready));
        let job = fixture(include_str!("../../tests/fixtures/helical.gcode"));
        h.cmd.send(Command::StartJob(job.clone())).unwrap();
        h.wait_for("some progress", |e| {
            matches!(e, Event::Job(JobEvent::Progress { done, .. }) if *done > 50)
        });
        h.cmd.send(Command::StopJob).unwrap();
        h.wait_for("stopped", |e| matches!(e, Event::Job(JobEvent::Stopped)));
        // Give the quick stop time to go out, then check no job moves followed it.
        std::thread::sleep(Duration::from_millis(200));
        let executed = h.executed.lock().unwrap().clone();
        let quick = executed.iter().position(|c| c == "M410").expect("M410 sent");
        assert!(
            executed[quick + 1..].iter().all(|c| !c.starts_with("G0")),
            "job moves sent after the stop"
        );
        assert!(h.executed_moves().len() < moves(&job).len());
    }
}
