//! Bug-report subcommand: capture a structured incident report and
//! POST it to the running gateway, which forwards to the upstream
//! feedback endpoint.
//!
//! Three input modes are supported:
//!   * **Flags** — all four required fields supplied on the CLI; submit immediately.
//!   * **Pipe** — stdin is not a TTY; read a markdown form from stdin and parse.
//!   * **Editor** — neither of the above; open `$EDITOR` on a templated form.

use std::io::{IsTerminal, Read, Write};

use residuum::util::FatalError;
use serde::Deserialize;

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
#[value(rename_all = "lower")]
pub(super) enum SeverityArg {
    Broken,
    Wrong,
    Annoying,
}

impl SeverityArg {
    fn as_wire(self) -> &'static str {
        match self {
            Self::Broken => "broken",
            Self::Wrong => "wrong",
            Self::Annoying => "annoying",
        }
    }
}

#[derive(clap::Args)]
pub(super) struct BugReportArgs {
    /// What actually happened (required)
    #[arg(long)]
    pub happened: Option<String>,
    /// What you expected to happen (required)
    #[arg(long)]
    pub expected: Option<String>,
    /// What you were doing when it happened (required)
    #[arg(long)]
    pub doing: Option<String>,
    /// Severity: one of broken, wrong, annoying
    #[arg(long, value_enum)]
    pub severity: Option<SeverityArg>,
    /// Backwards-compatible alias for --happened
    #[arg(short, long)]
    pub message: Option<String>,
}

#[derive(Deserialize)]
struct SubmissionReceipt {
    public_id: String,
    #[expect(dead_code, reason = "wire field; surfaced via debug log if needed")]
    submitted_at: String,
}

/// Send a bug report to the developer endpoint via the local gateway.
///
/// # Errors
/// Returns a `FatalError::Gateway` if the gateway is unreachable, the
/// upstream rejects the report, or the user-supplied input is invalid.
pub(super) async fn run_bug_report_command(
    args: &BugReportArgs,
    gateway_addr: &str,
) -> Result<(), FatalError> {
    let happened = args.happened.clone().or_else(|| args.message.clone());
    let body = match (
        happened.as_deref(),
        args.expected.as_deref(),
        args.doing.as_deref(),
        args.severity,
    ) {
        (Some(h), Some(e), Some(d), Some(s)) => build_body(h, e, d, s.as_wire()),
        _ if !std::io::stdin().is_terminal() => parse_markdown(&read_stdin()?)?,
        _ => parse_markdown(&open_editor()?)?,
    };

    let url = format!("http://{gateway_addr}/api/tracing/bug-report");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .map_err(|e| FatalError::Gateway(format!("failed to build HTTP client: {e}")))?;

    let resp = client.post(&url).json(&body).send().await.map_err(|e| {
        FatalError::Gateway(format!(
            "failed to reach gateway at {gateway_addr}: {e}\nhint: is the gateway running? try 'residuum serve'"
        ))
    })?;

    let status = resp.status();
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        return Err(FatalError::Gateway(format!(
            "bug report failed ({status}): {}",
            if body_text.is_empty() {
                "no response body".to_string()
            } else {
                body_text
            }
        )));
    }

    let receipt: SubmissionReceipt = resp
        .json()
        .await
        .map_err(|e| FatalError::Gateway(format!("failed to parse bug report response: {e}")))?;

    println!("Bug report submitted: {}", receipt.public_id);
    println!("Reference this ID in a GitHub issue if you have more to add.");
    Ok(())
}

fn build_body(happened: &str, expected: &str, doing: &str, severity: &str) -> serde_json::Value {
    serde_json::json!({
        "what_happened": happened,
        "what_expected": expected,
        "what_doing": doing,
        "severity": severity,
    })
}

fn read_stdin() -> Result<String, FatalError> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| FatalError::Gateway(format!("failed to read stdin: {e}")))?;
    Ok(buf)
}

fn open_editor() -> Result<String, FatalError> {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .map_err(|_unset| {
            FatalError::Gateway(
            "no $EDITOR or $VISUAL set; provide --happened/--expected/--doing/--severity instead"
                .to_string(),
        )
        })?;

    let template = editor_template();
    let tmp = tempfile::Builder::new()
        .prefix("residuum-bug-report-")
        .suffix(".md")
        .tempfile()
        .map_err(|e| FatalError::Gateway(format!("failed to create temp file: {e}")))?;
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(tmp.path())
            .map_err(|e| FatalError::Gateway(format!("failed to open temp file: {e}")))?;
        f.write_all(template.as_bytes())
            .map_err(|e| FatalError::Gateway(format!("failed to write temp file: {e}")))?;
    }

    let status = std::process::Command::new(&editor)
        .arg(tmp.path())
        .status()
        .map_err(|e| FatalError::Gateway(format!("failed to launch editor '{editor}': {e}")))?;
    if !status.success() {
        return Err(FatalError::Gateway(format!(
            "editor '{editor}' exited with status {status}; aborting bug report"
        )));
    }

    std::fs::read_to_string(tmp.path())
        .map_err(|e| FatalError::Gateway(format!("failed to read edited file: {e}")))
}

fn editor_template() -> String {
    let version = env!("RESIDUUM_VERSION");
    let commit = option_env!("RESIDUUM_GIT_COMMIT").unwrap_or("(unknown)");
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    format!(
        "<!--
Auto-attached to this report (read-only):
  version:  {version}
  commit:   {commit}
  os/arch:  {os}/{arch}
  (model and trace context are added server-side)

Fill in each section below. Comments are ignored on save.
Severity must be one of: broken, wrong, annoying.
-->

## What happened?


## What did you expect?


## What were you doing?


## Severity

"
    )
}

/// Parse the four-section markdown into a request body.
///
/// Splits on `## ` headings, ignoring HTML comment blocks. Each section
/// must be present and non-empty after trimming.
fn parse_markdown(text: &str) -> Result<serde_json::Value, FatalError> {
    let mut sections: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut current: Option<String> = None;
    let mut buf = String::new();
    let mut in_comment = false;

    for line in text.lines() {
        if in_comment {
            if line.contains("-->") {
                in_comment = false;
            }
            continue;
        }
        if let Some(rest) = line.trim_start().strip_prefix("<!--") {
            if !rest.contains("-->") {
                in_comment = true;
            }
            continue;
        }
        if let Some(heading) = line.strip_prefix("## ") {
            if let Some(name) = current.take() {
                sections.insert(name, buf.trim().to_string());
                buf.clear();
            }
            current = Some(normalize_heading(heading));
        } else if current.is_some() {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    if let Some(name) = current.take() {
        sections.insert(name, buf.trim().to_string());
    }

    let happened = sections
        .get("what happened?")
        .or_else(|| sections.get("what happened"))
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            FatalError::Gateway("missing or empty `## What happened?` section".to_string())
        })?;
    let expected = sections
        .get("what did you expect?")
        .or_else(|| sections.get("what did you expect"))
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            FatalError::Gateway("missing or empty `## What did you expect?` section".to_string())
        })?;
    let doing = sections
        .get("what were you doing?")
        .or_else(|| sections.get("what were you doing"))
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            FatalError::Gateway("missing or empty `## What were you doing?` section".to_string())
        })?;
    let severity_raw = sections
        .get("severity")
        .filter(|v| !v.is_empty())
        .ok_or_else(|| FatalError::Gateway("missing or empty `## Severity` section".to_string()))?;
    let severity = match severity_raw.trim().to_ascii_lowercase().as_str() {
        "broken" => "broken",
        "wrong" => "wrong",
        "annoying" => "annoying",
        other => {
            return Err(FatalError::Gateway(format!(
                "invalid severity '{other}': must be broken, wrong, or annoying"
            )));
        }
    };

    Ok(build_body(happened, expected, doing, severity))
}

fn normalize_heading(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::indexing_slicing,
    reason = "test code indexes serde_json::Value by known-present keys"
)]
mod tests {
    use super::*;

    #[test]
    fn parse_markdown_happy_path() {
        let text = "## What happened?\nIt crashed\n\n## What did you expect?\nNo crash\n\n## What were you doing?\nClicking\n\n## Severity\nbroken\n";
        let body = parse_markdown(text).unwrap();
        assert_eq!(body["what_happened"], "It crashed");
        assert_eq!(body["what_expected"], "No crash");
        assert_eq!(body["what_doing"], "Clicking");
        assert_eq!(body["severity"], "broken");
    }

    #[test]
    fn parse_markdown_strips_html_comments() {
        let text = "<!-- preamble -->\n## What happened?\nA\n## What did you expect?\nB\n## What were you doing?\nC\n## Severity\nwrong\n";
        let body = parse_markdown(text).unwrap();
        assert_eq!(body["what_happened"], "A");
        assert_eq!(body["severity"], "wrong");
    }

    #[test]
    fn parse_markdown_strips_multiline_html_comments() {
        let text = "<!--\nmany lines\nof preamble\n-->\n## What happened?\nA\n## What did you expect?\nB\n## What were you doing?\nC\n## Severity\nannoying\n";
        let body = parse_markdown(text).unwrap();
        assert_eq!(body["what_happened"], "A");
        assert_eq!(body["severity"], "annoying");
    }

    #[test]
    fn parse_markdown_rejects_missing_section() {
        let text = "## What happened?\nA\n## What did you expect?\nB\n## Severity\nbroken\n";
        let err = parse_markdown(text).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("What were you doing"), "got: {msg}");
    }

    #[test]
    fn parse_markdown_rejects_invalid_severity() {
        let text = "## What happened?\nA\n## What did you expect?\nB\n## What were you doing?\nC\n## Severity\nslow\n";
        let err = parse_markdown(text).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("invalid severity"), "got: {msg}");
    }

    #[test]
    fn severity_arg_wire_values_lowercase() {
        assert_eq!(SeverityArg::Broken.as_wire(), "broken");
        assert_eq!(SeverityArg::Wrong.as_wire(), "wrong");
        assert_eq!(SeverityArg::Annoying.as_wire(), "annoying");
    }
}
