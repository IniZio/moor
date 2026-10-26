//! Rule matching and reply playback for `ScriptedProc`, including `run_reading`.

use std::io::Read;

use moor_core::seam::proc::{ProcError, ProcOutput, ProcSpec, OUTPUT_CAP as CAP};

use super::{poisoned, Reply, ScriptedProc};

/// Read a lazy reader to its end.
pub(super) fn drain(open: fn() -> Box<dyn Read + Send>) -> Vec<u8> {
    let mut bytes = Vec::new();
    // An in-memory or generated reader has no failure to report.
    let _ = open().read_to_end(&mut bytes);
    bytes
}

impl ScriptedProc {
    /// The first matching rule's reply; a `once` rule is dropped by the match.
    fn pick(&self, spec: &ProcSpec, args: &[String]) -> Result<Option<Reply>, ProcError> {
        let mut rules = self.rules.lock().map_err(|_| poisoned())?;
        let at = rules
            .iter()
            .position(|r| r.tool == spec.tool && args.starts_with(&r.prefix));
        let taken = at
            .filter(|i| rules.get(*i).is_some_and(|r| r.once))
            .map(|i| rules.remove(i));
        let hit = taken.as_ref().or_else(|| at.and_then(|i| rules.get(i)));
        let reply = hit.map(|r| r.reply.clone());
        drop(rules);
        Ok(match reply {
            Some(Reply::Computed(f)) => Some(f(args)),
            other => other,
        })
    }

    /// The error for a call no rule matched.
    fn unscripted(spec: &ProcSpec, args: &[String]) -> ProcError {
        ProcError::NonZero {
            code: 127,
            stdout: String::new(),
            stderr_tail: format!("unscripted: {} {}", spec.tool.program(), args.join(" ")),
        }
    }

    /// Resolve a call to its scripted output or error.
    pub(super) fn reply_for(
        &self,
        spec: &ProcSpec,
        args: &[String],
    ) -> Result<ProcOutput, ProcError> {
        let ok = |mut stdout: String| {
            let truncated = stdout.len() > CAP;
            let mut at = CAP.min(stdout.len());
            while !stdout.is_char_boundary(at) {
                at -= 1;
            }
            stdout.truncate(at);
            Ok(ProcOutput {
                code: 0,
                stdout,
                stderr: String::new(),
                truncated,
            })
        };
        match self.pick(spec, args)? {
            Some(Reply::Ok(out)) => ok(out),
            Some(Reply::Bytes(b)) => ok(String::from_utf8_lossy(&b).into_owned()),
            Some(Reply::Reader(open)) => ok(String::from_utf8_lossy(&drain(open)).into_owned()),
            Some(Reply::Fail {
                code,
                stdout,
                stderr,
            }) => Err(ProcError::NonZero {
                code,
                stdout,
                stderr_tail: stderr,
            }),
            Some(Reply::Signalled(sig)) => Err(ProcError::Signalled(sig)),
            Some(Reply::Interrupted) => Err(ProcError::Interrupted),
            Some(Reply::Timeout) => Err(ProcError::Timeout {
                tool: spec.tool,
                after: spec.deadline.0,
            }),
            Some(Reply::Computed(_)) | None => Err(Self::unscripted(spec, args)),
        }
    }

    /// Hand the scripted stdout to `read`, then report the scripted outcome.
    pub(super) fn reading(
        &self,
        spec: &ProcSpec,
        read: &mut dyn FnMut(&mut dyn Read),
    ) -> Result<(), ProcError> {
        let args = self.record(spec);
        let (stdout, end): (Box<dyn Read + Send>, Result<(), ProcError>) = match self
            .pick(spec, &args)?
        {
            Some(Reply::Ok(out)) => (Box::new(std::io::Cursor::new(out.into_bytes())), Ok(())),
            Some(Reply::Bytes(b)) => (Box::new(std::io::Cursor::new(b)), Ok(())),
            Some(Reply::Reader(open)) => (open(), Ok(())),
            Some(Reply::Fail {
                code,
                stdout,
                stderr,
            }) => (
                Box::new(std::io::Cursor::new(stdout.into_bytes())),
                Err(ProcError::NonZero {
                    code,
                    stdout: String::new(),
                    stderr_tail: stderr,
                }),
            ),
            Some(Reply::Signalled(sig)) => {
                (Box::new(std::io::empty()), Err(ProcError::Signalled(sig)))
            }
            Some(Reply::Interrupted) => (Box::new(std::io::empty()), Err(ProcError::Interrupted)),
            Some(Reply::Timeout) => (
                Box::new(std::io::empty()),
                Err(ProcError::Timeout {
                    tool: spec.tool,
                    after: spec.deadline.0,
                }),
            ),
            Some(Reply::Computed(_)) | None => (
                Box::new(std::io::empty()),
                Err(Self::unscripted(spec, &args)),
            ),
        };
        let mut stdout = stdout;
        read(&mut *stdout);
        end
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use moor_core::seam::proc::{Deadline, Proc, Tool};

    use super::*;

    fn spec(args: &[&str]) -> ProcSpec {
        ProcSpec::new(Tool::Ssh, Deadline(Duration::from_secs(2))).args(args.iter().copied())
    }

    fn slurp(p: &ScriptedProc, args: &[&str]) -> (Vec<u8>, Result<(), ProcError>) {
        let mut got = Vec::new();
        let end = p.run_reading(&spec(args), &mut |r| {
            let _ = r.read_to_end(&mut got);
        });
        (got, end)
    }

    #[test]
    fn bytes_and_ok_stream_then_succeed() {
        let p = ScriptedProc::new();
        p.on(Tool::Ssh, &["b"], Reply::Bytes(vec![0, 159, 1])).on(
            Tool::Ssh,
            &["o"],
            Reply::Ok("hi".into()),
        );
        assert_eq!(slurp(&p, &["b"]).0, vec![0, 159, 1]);
        let (got, end) = slurp(&p, &["o"]);
        assert_eq!(got, b"hi");
        assert!(end.is_ok());
        assert_eq!(p.calls().len(), 2);
    }

    #[test]
    fn reader_is_lazy_and_may_stop_early() {
        const BIG: u64 = 50 * 1024 * 1024;
        let p = ScriptedProc::new();
        p.on(
            Tool::Ssh,
            &["big"],
            Reply::Reader(|| Box::new(std::io::repeat(7).take(BIG))),
        );
        let mut head = [0u8; 4];
        let end = p.run_reading(&spec(&["big"]), &mut |r| {
            r.read_exact(&mut head).unwrap();
        });
        assert!(end.is_ok());
        assert_eq!(head, [7; 4]);
        let mut total = 0u64;
        p.run_reading(&spec(&["big"]), &mut |r| {
            total = std::io::copy(r, &mut std::io::sink()).unwrap();
        })
        .unwrap();
        assert_eq!(total, BIG);
    }

    #[test]
    fn fail_reads_stdout_then_errors_without_it() {
        let p = ScriptedProc::new();
        p.on(
            Tool::Ssh,
            &["f"],
            Reply::Fail {
                code: 2,
                stdout: "partial".into(),
                stderr: "boom".into(),
            },
        );
        let (got, end) = slurp(&p, &["f"]);
        assert_eq!(got, b"partial");
        assert!(matches!(
            end,
            Err(ProcError::NonZero { code: 2, ref stdout, ref stderr_tail })
                if stdout.is_empty() && stderr_tail == "boom"
        ));
    }

    #[test]
    fn timeout_and_unscripted_read_nothing() {
        let p = ScriptedProc::new();
        p.on(Tool::Ssh, &["t"], Reply::Timeout);
        let (got, end) = slurp(&p, &["t"]);
        assert!(got.is_empty());
        assert!(matches!(end, Err(ProcError::Timeout { .. })));
        let (got, end) = slurp(&p, &["x"]);
        assert!(got.is_empty());
        assert!(matches!(
            end,
            Err(ProcError::NonZero { code: 127, ref stderr_tail, .. })
                if stderr_tail == "unscripted: ssh x"
        ));
    }

    #[test]
    fn run_drains_reader() {
        let p = ScriptedProc::new();
        p.on(Tool::Ssh, &["r"], Reply::Reader(|| Box::new(&b"abc"[..])));
        assert_eq!(p.run(&spec(&["r"])).unwrap().stdout, "abc");
    }
}
