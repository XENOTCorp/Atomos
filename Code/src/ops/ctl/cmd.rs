//! Command parse: words and JSON.
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    Help,
    Status,
    KeysList { reveal: bool },
    KeysAdd { note: String },
    KeysDel { index: usize, yes: bool },
    JsonDump,
    ConfigShow,
    Restart { yes: bool },
    Stop { yes: bool },
    Start,
    Refresh,
    Backup,
    DryTest,
    Quit,
}
pub fn parse_line(line: &str) -> Result<Cmd, String> {
    let t = line.trim();
    if t.is_empty() {
        return Err("empty".into());
    }
    if t.starts_with('{') {
        let v: Value = serde_json::from_str(t).map_err(|e| e.to_string())?;
        return parse_json(&v);
    }
    let words: Vec<&str> = t.split_whitespace().collect();
    parse_words(&words)
}

pub fn parse_words<S: AsRef<str>>(words: &[S]) -> Result<Cmd, String> {
    let mut yes = false;
    let mut reveal = false;
    let mut w: Vec<&str> = Vec::new();
    for x in words {
        match x.as_ref() {
            "--yes" | "-y" => yes = true,
            "--reveal" => reveal = true,
            s => w.push(s),
        }
    }
    if w.is_empty() {
        return Err("empty".into());
    }
    match w[0] {
        "help" | "-h" | "--help" => Ok(Cmd::Help),
        "status" | "ping" => Ok(Cmd::Status),
        "quit" | "exit" | "q" => Ok(Cmd::Quit),
        "config" => Ok(Cmd::ConfigShow),
        "json" => match w.get(1).copied().unwrap_or("dump") {
            "dump" | "pretty" | "show" => Ok(Cmd::JsonDump),
            "list" | "ls" => Ok(Cmd::KeysList { reveal }),
            other => Err(format!("unknown json subcommand: {other}")),
        },
        "keys" => {
            let sub = w.get(1).copied().unwrap_or("list");
            match sub {
                "list" | "ls" => Ok(Cmd::KeysList { reveal }),
                "add" => Ok(Cmd::KeysAdd {
                    note: w.get(2..).unwrap_or(&[]).join(" "),
                }),
                "del" | "rm" | "delete" => {
                    let index = w
                        .get(2)
                        .ok_or_else(|| "keys del <index>".to_string())?
                        .parse::<usize>()
                        .map_err(|_| "keys del <index> must be a number".to_string())?;
                    Ok(Cmd::KeysDel { index, yes })
                }
                other => Err(format!("unknown keys subcommand: {other}")),
            }
        }
        "restart" => Ok(Cmd::Restart { yes }),
        "stop" => Ok(Cmd::Stop { yes }),
        "start" => Ok(Cmd::Start),
        "refresh" | "refresh-endpoints" => Ok(Cmd::Refresh),
        "backup" => Ok(Cmd::Backup),
        "dry-test" | "dry_test" | "dry-test-rules" => Ok(Cmd::DryTest),
        other => Err(format!("unknown command: {other} (try help)")),
    }
}

pub fn parse_json(v: &Value) -> Result<Cmd, String> {
    let cmd = v
        .get("cmd")
        .and_then(|c| c.as_str())
        .ok_or_else(|| "missing cmd".to_string())?;
    let yes = v.get("yes").and_then(|x| x.as_bool()).unwrap_or(false);
    let reveal = v.get("reveal").and_then(|x| x.as_bool()).unwrap_or(false);
    let note = v
        .get("note")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let index = v.get("index").and_then(|x| x.as_u64()).map(|n| n as usize);
    match cmd {
        "help" => Ok(Cmd::Help),
        "status" | "ping" => Ok(Cmd::Status),
        "quit" | "exit" => Ok(Cmd::Quit),
        "config" => Ok(Cmd::ConfigShow),
        "json.dump" | "json_dump" | "json" => Ok(Cmd::JsonDump),
        "keys.list" | "keys_list" => Ok(Cmd::KeysList { reveal }),
        "keys.add" | "keys_add" => Ok(Cmd::KeysAdd { note }),
        "keys.del" | "keys_del" => Ok(Cmd::KeysDel {
            index: index.ok_or_else(|| "keys.del needs index".to_string())?,
            yes,
        }),
        "keys" => match v.get("op").and_then(|o| o.as_str()).unwrap_or("list") {
            "list" => Ok(Cmd::KeysList { reveal }),
            "add" => Ok(Cmd::KeysAdd { note }),
            "del" => Ok(Cmd::KeysDel {
                index: index.ok_or_else(|| "keys del needs index".to_string())?,
                yes,
            }),
            other => Err(format!("unknown keys op: {other}")),
        },
        "restart" => Ok(Cmd::Restart { yes }),
        "stop" => Ok(Cmd::Stop { yes }),
        "start" => Ok(Cmd::Start),
        "refresh" | "refresh-endpoints" => Ok(Cmd::Refresh),
        "backup" => Ok(Cmd::Backup),
        "dry-test" | "dry_test" | "dry-test-rules" => Ok(Cmd::DryTest),
        other => Err(format!("unknown cmd: {other}")),
    }
}
