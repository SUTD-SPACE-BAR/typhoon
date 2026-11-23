//! Golden tests: compare against output recorded from cyclone itself.
//!
//! `tests/fixtures/<name>.gcode` and `<name>.log` were produced by running
//! cyclone's own `planWind` (compiled from its TypeScript sources) on
//! `<name>.wind`, the same way its `plan` CLI command does. The `.log` holds
//! what cyclone printed to the console, with `WARN ` in front of warnings.

use super::*;

struct Fixture {
    name: &'static str,
    wind: &'static str,
    gcode: &'static str,
    log: &'static str,
    verbose: bool,
}

macro_rules! fixture {
    ($name:literal, $wind:literal, $verbose:expr) => {
        Fixture {
            name: $name,
            wind: include_str!(concat!("../../tests/fixtures/", $wind, ".wind")),
            gcode: include_str!(concat!("../../tests/fixtures/", $name, ".gcode")),
            log: include_str!(concat!("../../tests/fixtures/", $name, ".log")),
            verbose: $verbose,
        }
    };
}

fn fixtures() -> Vec<Fixture> {
    vec![
        fixture!("hoop", "hoop", false),
        fixture!("hoop_verbose", "hoop", true),
        fixture!("helical", "helical", false),
        fixture!("mixed", "mixed", false),
        fixture!("bad_pattern", "bad_pattern", false),
    ]
}

fn plan(f: &Fixture) -> PlanResult {
    let params = WindParameters::from_json(f.wind).unwrap_or_else(|e| panic!("{}: {e}", f.name));
    plan_wind(&params, f.verbose)
}

#[test]
fn gcode_matches_cyclone_exactly() {
    for f in fixtures() {
        let ours = plan(&f).gcode_text();
        // Fixtures may be checked out with CRLF line endings on Windows.
        let theirs = f.gcode.replace("\r\n", "\n");
        if ours != theirs {
            let ours_lines: Vec<_> = ours.lines().collect();
            let theirs_lines: Vec<_> = theirs.lines().collect();
            for (i, (a, b)) in ours_lines.iter().zip(&theirs_lines).enumerate() {
                assert_eq!(a, b, "{}: first difference at line {}", f.name, i + 1);
            }
            panic!(
                "{}: {} lines from us vs {} from cyclone",
                f.name,
                ours_lines.len(),
                theirs_lines.len()
            );
        }
    }
}

/// Console output must match too. Time and tow numbers come from summing
/// square roots, so they are compared with a tiny relative tolerance in case
/// the last bit differs; every other line must match exactly.
#[test]
fn log_matches_cyclone() {
    for f in fixtures() {
        let result = plan(&f);
        let ours: String = result
            .log
            .iter()
            .map(|l| match l.level {
                LogLevel::Info => l.text.clone(),
                LogLevel::Warn => format!("WARN {}", l.text),
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let theirs = f.log.replace("\r\n", "\n");
        let ours_lines: Vec<_> = ours.lines().collect();
        let theirs_lines: Vec<_> = theirs.lines().collect();
        assert_eq!(ours_lines.len(), theirs_lines.len(), "{}: log length", f.name);
        for (i, (a, b)) in ours_lines.iter().zip(&theirs_lines).enumerate() {
            if a == b {
                continue;
            }
            let (na, nb) = (stat_number(a), stat_number(b));
            match (na, nb) {
                (Some((pa, va)), Some((pb, vb))) if pa == pb => {
                    let tol = 1e-9 * vb.abs().max(1.0);
                    assert!(
                        (va - vb).abs() <= tol,
                        "{}: log line {} differs: {a:?} vs {b:?}",
                        f.name,
                        i + 1
                    );
                }
                _ => panic!("{}: log line {} differs: {a:?} vs {b:?}", f.name, i + 1),
            }
        }
    }
}

/// Splits "Layer time estimate: 12.5 seconds" into ("Layer time estimate:", 12.5).
fn stat_number(line: &str) -> Option<(&str, f64)> {
    let (prefix, rest) = line.rsplit_once(": ")?;
    let number = rest.split_whitespace().next()?.parse().ok()?;
    Some((prefix, number))
}

#[test]
fn summary_matches_log_totals() {
    let f = fixture!("mixed", "mixed", false);
    let result = plan(&f);
    // Layer 6 follows a terminal layer, so only five are planned.
    assert_eq!(result.layers.len(), 5);
    let sum: f64 = result.layers.iter().map(|l| l.time_s).sum();
    assert!((sum - result.total_time_s).abs() < 1e-6);
    assert_eq!(result.warnings().count(), 1);
}

#[test]
fn wind_file_round_trips() {
    for f in fixtures() {
        let params = WindParameters::from_json(f.wind).unwrap();
        let again = WindParameters::from_json(&params.to_json_pretty()).unwrap();
        assert_eq!(params, again, "{}", f.name);
    }
}
