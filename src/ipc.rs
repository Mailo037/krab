//! Single-instance hand-off. When Krab already runs in the background, a new
//! `krab` invocation just forwards its request over a local socket (a named
//! pipe on Windows) and exits - that round trip takes about a millisecond.

use crate::overlay::{Kind, Mode};
use interprocess::local_socket::{
    prelude::*, GenericFilePath, GenericNamespaced, ListenerOptions, Name, Stream,
};
use std::io::{self, BufRead, BufReader, Write};

/// What to capture and how.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    /// `None` = the configured default mode.
    pub mode: Option<Mode>,
    pub kind: Kind,
    /// Seconds to wait before capturing.
    pub delay: u32,
    /// Override `after_capture`: `Some(true)` copies immediately.
    pub quick: Option<bool>,
    /// Copy the text in the selected area instead of the picture.
    pub text: bool,
    /// Scroll the selected area and stitch it into one tall image.
    pub scroll: bool,
    /// Start with the window that has the focus selected.
    pub active: bool,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            mode: None,
            kind: Kind::Snip,
            delay: 0,
            quick: None,
            text: false,
            scroll: false,
            active: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Snip(Request),
    Ping,
    Quit,
    /// Open the settings window.
    Settings,
    /// The program at this path was just updated: restart into it, but only
    /// if that is the program this instance runs from.
    Updated(String),
}

impl Message {
    pub fn encode(&self) -> String {
        match self {
            Message::Ping => "ping".into(),
            Message::Quit => "quit".into(),
            Message::Settings => "settings".into(),
            Message::Updated(path) => format!("updated {path}"),
            Message::Snip(r) => format!(
                "snip mode={} kind={} delay={} quick={} text={} scroll={} active={}",
                r.mode.map(Mode::as_str).unwrap_or("default"),
                if r.kind == Kind::Record {
                    "record"
                } else {
                    "snip"
                },
                r.delay,
                match r.quick {
                    Some(true) => "yes",
                    Some(false) => "no",
                    None => "default",
                },
                if r.text { "yes" } else { "no" },
                if r.scroll { "yes" } else { "no" },
                if r.active { "yes" } else { "no" },
            ),
        }
    }

    pub fn decode(line: &str) -> Option<Message> {
        let mut parts = line.split_whitespace();
        match parts.next()? {
            "ping" => Some(Message::Ping),
            "quit" => Some(Message::Quit),
            "settings" => Some(Message::Settings),
            "updated" => {
                let path = line.trim_start().strip_prefix("updated")?.trim();
                (!path.is_empty()).then(|| Message::Updated(path.to_string()))
            }
            "snip" => {
                let mut r = Request::default();
                for kv in parts {
                    let (k, v) = kv.split_once('=')?;
                    match k {
                        "mode" => r.mode = Mode::parse(v),
                        "kind" => {
                            r.kind = if v == "record" {
                                Kind::Record
                            } else {
                                Kind::Snip
                            }
                        }
                        "delay" => r.delay = v.parse().ok()?,
                        "text" => r.text = v == "yes",
                        "scroll" => r.scroll = v == "yes",
                        "active" => r.active = v == "yes",
                        "quick" => {
                            r.quick = match v {
                                "yes" => Some(true),
                                "no" => Some(false),
                                _ => None,
                            }
                        }
                        _ => {}
                    }
                }
                Some(Message::Snip(r))
            }
            _ => None,
        }
    }
}

fn socket_name() -> io::Result<Name<'static>> {
    let user: String = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let name = format!("krab-v1-{user}.sock");
    if GenericNamespaced::is_supported() {
        name.to_ns_name::<GenericNamespaced>()
    } else {
        std::env::temp_dir()
            .join(name)
            .to_fs_name::<GenericFilePath>()
    }
}

/// Deliver `msg` to the running instance. Fails fast when there is none.
pub fn send(msg: &Message) -> io::Result<()> {
    let mut conn = Stream::connect(socket_name()?)?;
    conn.write_all(format!("{}\n", msg.encode()).as_bytes())?;
    conn.flush()?;
    // Wait for the acknowledgement so we know the request was really taken.
    let mut ack = String::new();
    BufReader::new(conn).read_line(&mut ack)?;
    if ack.trim() == "ok" {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected reply",
        ))
    }
}

/// Start serving requests on a background thread.
pub fn listen(on_message: impl Fn(Message) + Send + 'static) -> io::Result<()> {
    let listener = ListenerOptions::new()
        .name(socket_name()?)
        .try_overwrite(true)
        .create_sync()?;
    std::thread::Builder::new()
        .name("krab-ipc".into())
        .spawn(move || {
            for conn in listener.incoming().flatten() {
                let mut reader = BufReader::new(conn);
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let msg = Message::decode(line.trim());
                let _ = reader
                    .get_mut()
                    .write_all(if msg.is_some() { b"ok\n" } else { b"err\n" });
                if let Some(msg) = msg {
                    on_message(msg);
                }
            }
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_roundtrip() {
        let cases = [
            Message::Ping,
            Message::Quit,
            Message::Settings,
            Message::Updated("C:\\Users\\a b\\AppData\\Local\\Programs\\Krab\\krab.exe".into()),
            Message::Snip(Request::default()),
            Message::Snip(Request {
                mode: Some(Mode::Window),
                kind: Kind::Record,
                delay: 5,
                quick: Some(true),
                text: false,
                scroll: false,
                active: false,
            }),
            Message::Snip(Request {
                mode: Some(Mode::Window),
                active: true,
                ..Request::default()
            }),
            Message::Snip(Request {
                text: true,
                ..Request::default()
            }),
            Message::Snip(Request {
                scroll: true,
                ..Request::default()
            }),
            Message::Snip(Request {
                mode: Some(Mode::Freeform),
                kind: Kind::Snip,
                delay: 0,
                quick: Some(false),
                text: false,
                scroll: false,
                active: false,
            }),
        ];
        for m in cases {
            assert_eq!(Message::decode(&m.encode()), Some(m));
        }
    }

    #[test]
    fn junk_is_rejected() {
        assert_eq!(Message::decode(""), None);
        assert_eq!(Message::decode("format c:"), None);
        assert_eq!(Message::decode("snip delay=soon"), None);
    }
}
