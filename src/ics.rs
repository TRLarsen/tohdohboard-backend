//! A minimal iCalendar (RFC 5545) writer for the calendar feed.
//!
//! [`calendar`] turns the board into one all-day `VEVENT` per open task with
//! a `dueDate`. Events, not `VTODO`s, because calendar apps such as Google
//! Calendar only show events. Output uses CRLF line endings, folds lines at
//! 75 octets without splitting UTF-8 characters, and escapes `TEXT` values.

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;

use crate::archive::task_id;

/// Renders the board's due tasks as a `VCALENDAR`.
///
/// Skips tasks that are not objects, have no string `id`, are in
/// `done_column`, or lack a valid `dueDate` (`YYYY-MM-DD`). Archived tasks are
/// not on the board, so they never appear. `now` becomes every `DTSTAMP`.
pub fn calendar(board: &Value, done_column: &str, now: DateTime<Utc>) -> String {
    let mut out = String::new();
    for line in [
        "BEGIN:VCALENDAR",
        "VERSION:2.0",
        "PRODID:-//TohdohBoard//tohdohboard-server//EN",
        "CALSCALE:GREGORIAN",
        "X-WR-CALNAME:TohdohBoard",
    ] {
        push_line(&mut out, line);
    }
    let stamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let tasks = board.get("tasks").and_then(Value::as_array);
    for task in tasks.into_iter().flatten() {
        let Some(id) = task_id(task) else { continue };
        if task.get("status").and_then(Value::as_str) == Some(done_column) {
            continue;
        }
        let Some(due) = task
            .get("dueDate")
            .and_then(Value::as_str)
            .and_then(parse_date)
        else {
            continue;
        };
        let Some(end) = due.succ_opt() else { continue };
        let text = |field| task.get(field).and_then(Value::as_str).unwrap_or("");
        push_line(&mut out, "BEGIN:VEVENT");
        push_line(
            &mut out,
            &format!("UID:{}", escape_text(&format!("{id}@tohdohboard"))),
        );
        push_line(&mut out, &format!("DTSTAMP:{stamp}"));
        push_line(
            &mut out,
            &format!("DTSTART;VALUE=DATE:{}", due.format("%Y%m%d")),
        );
        push_line(
            &mut out,
            &format!("DTEND;VALUE=DATE:{}", end.format("%Y%m%d")),
        );
        push_line(&mut out, &format!("SUMMARY:{}", escape_text(text("title"))));
        let description = text("description");
        if !description.is_empty() {
            push_line(
                &mut out,
                &format!("DESCRIPTION:{}", escape_text(description)),
            );
        }
        push_line(&mut out, "END:VEVENT");
    }
    push_line(&mut out, "END:VCALENDAR");
    out
}

/// Parses a strict `YYYY-MM-DD` date: ASCII digits only, a valid calendar
/// date, and a year from 1 to 9998, so the day after still has four digits.
fn parse_date(s: &str) -> Option<NaiveDate> {
    let b = s.as_bytes();
    let digits = [0..4, 5..7, 8..10];
    let shape = b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && digits
            .into_iter()
            .all(|r| b[r].iter().all(u8::is_ascii_digit));
    if !shape {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s[r].parse::<u32>().ok();
    let year = i32::try_from(num(0..4)?)
        .ok()
        .filter(|y| (1..=9998).contains(y))?;
    NaiveDate::from_ymd_opt(year, num(5..7)?, num(8..10)?)
}

/// Escapes an RFC 5545 `TEXT` value: backslash, `;`, `,` and line breaks
/// (as `\n`). Other control characters, which `TEXT` may not contain, are
/// dropped.
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\r' => {
                chars.next_if_eq(&'\n');
                out.push_str("\\n");
            }
            '\n' => out.push_str("\\n"),
            '\t' => out.push('\t'),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Appends a content line, folded so no physical line exceeds 75 octets
/// (continuation lines start with a space), and terminated by CRLF.
pub fn push_line(out: &mut String, line: &str) {
    let mut len = 0;
    for c in line.chars() {
        if len + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            len = 1;
        }
        out.push(c);
        len += c.len_utf8();
    }
    out.push_str("\r\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-03-01T12:34:56Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn escapes_text() {
        assert_eq!(escape_text(r"a\b;c,d"), r"a\\b\;c\,d");
        assert_eq!(escape_text("1\n2\r\n3\r4"), r"1\n2\n3\n4");
        assert_eq!(escape_text("x\u{7}y\tz"), "xy\tz");
        assert_eq!(escape_text("caf\u{e9} \u{1f4c5}"), "caf\u{e9} \u{1f4c5}");
    }

    #[test]
    fn folds_at_75_octets_without_splitting_utf8() {
        let mut out = String::new();
        push_line(&mut out, &"a".repeat(75));
        assert_eq!(out, format!("{}\r\n", "a".repeat(75)));

        let mut out = String::new();
        let line = format!("SUMMARY:{}", "\u{e9}".repeat(100));
        push_line(&mut out, &line);
        assert!(out.ends_with("\r\n"));
        let physical: Vec<&str> = out.trim_end_matches("\r\n").split("\r\n").collect();
        assert!(physical.len() > 1);
        for (i, l) in physical.iter().enumerate() {
            assert!(l.len() <= 75, "line {i} has {} octets", l.len());
            assert_eq!(i > 0, l.starts_with(' '));
        }
        // Unfolding restores the original.
        assert_eq!(out.trim_end_matches("\r\n").replace("\r\n ", ""), line);
    }

    #[test]
    fn renders_open_tasks_with_due_dates() {
        let board = json!({ "tasks": [
            { "id": "t1", "title": "Pay rent, now; really", "description": "line1\nline2",
              "status": "todo", "dueDate": "2026-03-31" },
            { "id": "t2", "title": "No date", "status": "todo" },
            { "id": "t3", "title": "Done", "status": "done", "dueDate": "2026-03-05" },
            { "id": "t4", "title": "Bad", "status": "todo", "dueDate": "31.03.2026" },
            { "id": "t5", "title": "Short", "status": "todo", "dueDate": "2026-3-5" },
            { "id": "t6", "title": "Feb 30", "status": "todo", "dueDate": "2026-02-30" },
            { "title": "No id", "status": "todo", "dueDate": "2026-03-05" },
            { "id": "t7", "status": "doing", "dueDate": "2026-12-31", "description": "" }
        ] });
        let ics = calendar(&board, "done", now());
        assert!(ics.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"));
        assert!(ics.ends_with("END:VCALENDAR\r\n"));
        assert!(!ics.replace("\r\n", "").contains('\n'));
        assert_eq!(ics.matches("BEGIN:VEVENT").count(), 2);
        assert!(ics.contains(
            "BEGIN:VEVENT\r\nUID:t1@tohdohboard\r\nDTSTAMP:20260301T123456Z\r\n\
             DTSTART;VALUE=DATE:20260331\r\nDTEND;VALUE=DATE:20260401\r\n\
             SUMMARY:Pay rent\\, now\\; really\r\nDESCRIPTION:line1\\nline2\r\nEND:VEVENT\r\n"
        ));
        assert!(ics.contains(
            "UID:t7@tohdohboard\r\nDTSTAMP:20260301T123456Z\r\n\
             DTSTART;VALUE=DATE:20261231\r\nDTEND;VALUE=DATE:20270101\r\nSUMMARY:\r\nEND:VEVENT"
        ));
        assert!(ics.contains("X-WR-CALNAME:TohdohBoard\r\n"));
    }

    #[test]
    fn parses_only_strict_dates() {
        assert_eq!(
            parse_date("2026-03-31"),
            NaiveDate::from_ymd_opt(2026, 3, 31)
        );
        assert_eq!(parse_date("0001-01-01"), NaiveDate::from_ymd_opt(1, 1, 1));
        assert!(parse_date("9998-12-31").is_some());
        for bad in [
            "9999-12-31",
            "0000-01-01",
            "+026-03-31",
            " 026-03-31",
            "2026-3-031",
            "2026-03-3 ",
            "2026/03/31",
            "2026-02-30",
            "2026-13-01",
            "2026-00-10",
            "２０２６-03-31",
            "2026-03-31T00:00",
            "",
        ] {
            assert_eq!(parse_date(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn empty_or_odd_boards_give_an_empty_calendar() {
        for board in [json!({}), json!(null), json!({ "tasks": 3 })] {
            let ics = calendar(&board, "done", now());
            assert!(!ics.contains("VEVENT"));
            assert!(ics.ends_with("END:VCALENDAR\r\n"));
        }
    }
}
