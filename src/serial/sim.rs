//! A small simulated Marlin board, used as the "Simulator" port and in tests.
//!
//! It checks line numbers and checksums like Marlin (and can deliberately
//! report checksum errors to exercise resends), tracks X/Y/Z for G0/G1/G28/G92
//! in absolute or relative mode, answers M114/M115, and halts on M112.

use std::collections::{HashSet, VecDeque};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::Link;
use super::protocol::checksum;

pub struct SimMarlin {
    /// Bytes waiting to be read by the host.
    out: VecDeque<u8>,
    /// Replies scheduled for later (time, text), in order.
    pending: VecDeque<(Instant, String)>,
    inbuf: Vec<u8>,
    pos: [f64; 3],
    relative: bool,
    feed: f64,
    last_line: u32,
    halted: bool,
    /// Pretend every k-th numbered line arrived corrupted (once per line).
    corrupt_every: Option<u32>,
    corrupted: HashSet<u32>,
    /// Commands actually executed, in order (for tests).
    pub executed: Arc<Mutex<Vec<String>>>,
    /// Scale for simulated move time (0 = instant).
    time_scale: f64,
}

impl SimMarlin {
    pub fn new() -> Self {
        let mut sim = Self {
            out: VecDeque::new(),
            pending: VecDeque::new(),
            inbuf: Vec::new(),
            pos: [0.0; 3],
            relative: false,
            feed: 3000.0,
            last_line: 0,
            halted: false,
            corrupt_every: None,
            corrupted: HashSet::new(),
            executed: Arc::new(Mutex::new(Vec::new())),
            time_scale: 0.05,
        };
        sim.reply_after(Duration::from_millis(300), "start");
        sim.reply_after(Duration::ZERO, "echo:Marlin 2.0.9.2 (Typhoon simulator)");
        sim
    }

    /// For tests: report a checksum error on every `k`-th line, and run instantly.
    #[cfg(test)]
    pub fn with_faults(mut self, k: u32) -> Self {
        self.corrupt_every = Some(k);
        self.time_scale = 0.0;
        self
    }

    fn reply_after(&mut self, delay: Duration, text: impl Into<String>) {
        let earliest = self.pending.back().map(|(t, _)| *t).unwrap_or_else(Instant::now);
        let at = earliest.max(Instant::now()) + delay;
        self.pending.push_back((at, text.into()));
    }

    fn reply(&mut self, text: impl Into<String>) {
        self.reply_after(Duration::ZERO, text);
    }

    fn handle_line(&mut self, raw: &str) {
        let raw = raw.trim();
        if raw.is_empty() || self.halted {
            return;
        }
        // M112 is acted on immediately, numbered or not.
        if raw.contains("M112") {
            self.halted = true;
            self.pending.clear();
            self.reply("Error:Printer halted. kill() called!");
            return;
        }

        let command = if let Some(rest) = raw.strip_prefix('N') {
            let Some((body, cs)) = raw.rsplit_once('*') else {
                return self.resend("No Checksum with line number");
            };
            let n: u32 = match rest.split_whitespace().next().and_then(|s| s.parse().ok()) {
                Some(n) => n,
                None => return self.resend("Line number missing"),
            };
            let fault = self.corrupt_every.is_some_and(|k| n.is_multiple_of(k)) && self.corrupted.insert(n);
            if fault || cs.trim().parse::<u8>().ok() != Some(checksum(body)) {
                return self.resend("checksum mismatch");
            }
            let cmd = body.split_once(' ').map(|(_, c)| c.trim()).unwrap_or("");
            if cmd.starts_with("M110") {
                self.last_line = n;
            } else if n != self.last_line + 1 {
                return self.resend("Line Number is not Last Line Number+1");
            } else {
                self.last_line = n;
            }
            cmd.to_string()
        } else {
            raw.to_string()
        };

        self.execute(&command);
    }

    fn resend(&mut self, why: &str) {
        let last = self.last_line;
        self.reply(format!("Error:{why}, Last Line: {last}"));
        self.reply(format!("Resend: {}", last + 1));
        self.reply("ok");
    }

    fn execute(&mut self, command: &str) {
        self.executed.lock().unwrap().push(command.to_string());
        let mut words = command.split_whitespace();
        let code = words.next().unwrap_or("").to_ascii_uppercase();
        let args: Vec<(char, f64)> = words
            .filter_map(|w| {
                let (a, v) = w.split_at(1);
                Some((a.chars().next()?.to_ascii_uppercase(), v.parse().ok()?))
            })
            .collect();
        let get = |c: char| args.iter().find(|(a, _)| *a == c).map(|(_, v)| *v);
        let mut delay = Duration::from_millis(1);

        match code.as_str() {
            "G90" => self.relative = false,
            "G91" => self.relative = true,
            "G0" | "G1" => {
                if let Some(f) = get('F') {
                    self.feed = f.max(1.0);
                }
                let mut dist_sq = 0.0;
                for (i, axis) in ['X', 'Y', 'Z'].into_iter().enumerate() {
                    if let Some(v) = get(axis) {
                        let target = if self.relative { self.pos[i] + v } else { v };
                        dist_sq += (target - self.pos[i]).powi(2);
                        self.pos[i] = target;
                    }
                }
                let secs = dist_sq.sqrt() / self.feed * 60.0 * self.time_scale;
                delay = Duration::from_secs_f64(secs.min(2.0)).max(Duration::from_millis(1));
            }
            "G28" => {
                let any = ['X', 'Y', 'Z'].iter().any(|c| command.to_ascii_uppercase().contains(*c));
                for (i, axis) in ['X', 'Y', 'Z'].into_iter().enumerate() {
                    if !any || command.to_ascii_uppercase().contains(axis) {
                        self.pos[i] = 0.0;
                    }
                }
                if self.time_scale > 0.0 {
                    delay = Duration::from_millis(800);
                    self.reply_after(Duration::from_millis(400), "echo:busy: processing");
                }
            }
            "G92" => {
                for (i, axis) in ['X', 'Y', 'Z'].into_iter().enumerate() {
                    if let Some(v) = get(axis) {
                        self.pos[i] = v;
                    }
                }
            }
            "M114" => {
                let [x, y, z] = self.pos;
                self.reply(format!(
                    "X:{x:.2} Y:{y:.2} Z:{z:.2} E:0.00 Count X:{} Y:{} Z:{}",
                    (x * 80.0) as i64,
                    (y * 8.0) as i64,
                    (z * 8.0) as i64
                ));
            }
            "M115" => self.reply(
                "FIRMWARE_NAME:Marlin 2.0.9.2 (Typhoon simulator) SOURCE_CODE_URL:github.com/MarlinFirmware/Marlin PROTOCOL_VERSION:1.0 MACHINE_TYPE:Filament winder EXTRUDER_COUNT:0",
            ),
            "M110" => {
                if let Some(n) = get('N') {
                    self.last_line = n as u32;
                }
            }
            "M84" | "M18" | "M17" | "M400" | "M410" | "M220" | "M211" => {}
            _ => self.reply(format!("echo:Unknown command: \"{command}\"")),
        }
        self.reply_after(delay, "ok");
    }
}

impl Link for SimMarlin {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let now = Instant::now();
        while self.pending.front().is_some_and(|(t, _)| *t <= now) {
            let (_, text) = self.pending.pop_front().expect("checked");
            self.out.extend(text.as_bytes());
            self.out.push_back(b'\n');
        }
        if self.out.is_empty() {
            // Behave like a serial port with a short read timeout.
            std::thread::sleep(Duration::from_millis(2));
            return Ok(0);
        }
        let n = buf.len().min(self.out.len());
        for (i, b) in self.out.drain(..n).enumerate() {
            buf[i] = b;
        }
        Ok(n)
    }

    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        self.inbuf.extend_from_slice(data);
        while let Some(i) = self.inbuf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.inbuf.drain(..=i).collect();
            let line = String::from_utf8_lossy(&line).into_owned();
            self.handle_line(&line);
        }
        Ok(())
    }
}
