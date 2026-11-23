//! Marlin host protocol: numbered, checksummed lines out; `ok`, `Resend`,
//! errors and position reports back.

/// Machine position as reported by `M114` (X mm, Y and Z degrees for the winder).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Position {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Response {
    /// The last line was accepted (`ok`, possibly with extra fields).
    Ok,
    /// Marlin wants lines resent from this line number.
    Resend(u32),
    /// `Error:...` (text after the prefix).
    Error(String),
    /// `echo:busy: ...` keep-alive while a long command runs.
    Busy,
    /// The board (re)started.
    Start,
    /// An `M114` report.
    Position(Position),
    /// Anything else (echo lines, firmware info, ...).
    Other,
}

/// Marlin's checksum: XOR of every byte of the line before the `*`.
pub fn checksum(line: &str) -> u8 {
    line.bytes().fold(0, |acc, b| acc ^ b)
}

/// `N<n> <command>*<checksum>`, the form Marlin verifies and can ask to resend.
pub fn numbered(n: u32, command: &str) -> String {
    let body = format!("N{n} {command}");
    let cs = checksum(&body);
    format!("{body}*{cs}")
}

/// Drop `;` comments and surrounding whitespace. Empty means nothing to send.
pub fn clean_command(line: &str) -> &str {
    line.split(';').next().unwrap_or("").trim()
}

pub fn parse_response(line: &str) -> Response {
    let line = line.trim();
    if line == "ok" || line.starts_with("ok ") {
        return Response::Ok;
    }
    if let Some(rest) = line.strip_prefix("Resend:").or_else(|| line.strip_prefix("rs "))
        && let Ok(n) = rest.trim().parse() {
            return Response::Resend(n);
        }
    if let Some(rest) = line.strip_prefix("Error:") {
        return Response::Error(rest.trim().to_string());
    }
    if line.starts_with("echo:busy") {
        return Response::Busy;
    }
    if line == "start" {
        return Response::Start;
    }
    if let Some(p) = parse_position(line) {
        return Response::Position(p);
    }
    Response::Other
}

/// Parse `X:12.00 Y:0.00 Z:-5.00 E:0.00 Count X:...` (the part before `Count`).
fn parse_position(line: &str) -> Option<Position> {
    if !line.starts_with("X:") {
        return None;
    }
    let head = line.split(" Count").next().unwrap_or(line);
    let mut pos = Position::default();
    let mut seen = [false; 3];
    for token in head.split_whitespace() {
        let (axis, value) = token.split_once(':')?;
        let v: f64 = value.parse().ok()?;
        match axis {
            "X" => (pos.x, seen[0]) = (v, true),
            "Y" => (pos.y, seen[1]) = (v, true),
            "Z" => (pos.z, seen[2]) = (v, true),
            _ => {}
        }
    }
    seen.iter().all(|s| *s).then_some(pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_marlin() {
        // Known values (same XOR algorithm Marlin and Printrun use).
        assert_eq!(numbered(1, "G28"), "N1 G28*18");
        assert_eq!(checksum("N0 M110 N0"), 125);
        assert_eq!(numbered(42, "G0 X10 Y20.5"), format!("N42 G0 X10 Y20.5*{}", checksum("N42 G0 X10 Y20.5")));
    }

    #[test]
    fn cleans_comments() {
        assert_eq!(clean_command("G0 X1 ; move"), "G0 X1");
        assert_eq!(clean_command("; Layer 1 of 2: hoop"), "");
        assert_eq!(clean_command("   "), "");
    }

    #[test]
    fn parses_responses() {
        assert_eq!(parse_response("ok"), Response::Ok);
        assert_eq!(parse_response("ok N12 P15 B3"), Response::Ok);
        assert_eq!(parse_response("Resend: 6"), Response::Resend(6));
        assert_eq!(parse_response("Resend:7"), Response::Resend(7));
        assert_eq!(
            parse_response("Error:checksum mismatch, Last Line: 5"),
            Response::Error("checksum mismatch, Last Line: 5".into())
        );
        assert_eq!(parse_response("echo:busy: processing"), Response::Busy);
        assert_eq!(parse_response("start"), Response::Start);
        assert_eq!(parse_response("echo:Unknown command: \"G999\""), Response::Other);
        assert_eq!(
            parse_response("X:120.00 Y:6531.43 Z:-5.73 E:0.00 Count X:9600 Y:52251 Z:-46"),
            Response::Position(Position { x: 120.0, y: 6531.43, z: -5.73 })
        );
        assert_eq!(parse_response("X:1.00 Y:2.00"), Response::Other);
    }
}
