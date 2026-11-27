//! The streaming verbs, `exec` and `forward`, which carry frames rather than one answer.

use moor_client::link::boxes::{BoxAnswer, BoxOp};
use moor_client::link::ops;

use super::provider::Mark;
use super::{answer, inside, refused, valid_name, BoxProvider, Boxes, Wire};

impl<P: BoxProvider> Boxes<P> {
    /// Runs the command `op` asks for in its place and carries its frames on `stream`.
    pub(super) async fn exec<S: Wire>(&self, op: &BoxOp, mut stream: S) {
        let BoxOp::Exec {
            slug,
            place,
            argv,
            env,
            cwd,
            stdin,
            tty,
        } = op
        else {
            return;
        };
        let started = async {
            if argv.is_empty() {
                return Err("an exec needs a command".to_owned());
            }
            if let Some((name, _)) = env.iter().find(|(name, _)| !valid_name(name)) {
                return Err(format!("{name:?} is not a variable name"));
            }
            let Some(slug) = self.awake(slug)? else {
                return Ok(None);
            };
            let mark = Mark::new();
            let carrier = self.provider.carrier(&slug, *place, tty.is_some(), &mark);
            let ender = self.provider.ender(&slug, &mark);
            let script = inside::script(env, cwd.as_deref(), argv);
            inside::start(carrier, &script, *stdin, *tty)
                .map(|started| Some((started, ender)))
                .map_err(|e| format!("cannot start the command: {e}"))
        };
        match started.await {
            Ok(Some((started, ender))) => {
                answer(&mut stream, BoxAnswer::Streaming).await;
                inside::pump(started, *place, stream, ender).await;
            }
            Ok(None) => answer(&mut stream, BoxAnswer::Gone).await,
            Err(reason) => answer(&mut stream, refused(reason)).await,
        }
    }

    /// Whether anything listens on a port of the Box's loopback, or a stream to it.
    pub(super) async fn forward<S: Wire>(&self, op: &BoxOp, mut stream: S) {
        let BoxOp::Forward { slug, port, probe } = op else {
            return;
        };
        let slug = match self.awake(slug) {
            Ok(Some(slug)) => slug,
            Ok(None) => return answer(&mut stream, BoxAnswer::Gone).await,
            Err(reason) => return answer(&mut stream, refused(reason)).await,
        };
        let mark = Mark::new();
        let carrier =
            self.provider
                .carrier(&slug, moor_core::seam::driver::Place::Agent, false, &mark);
        if *probe {
            let script = format!(
                "exec bash -c {}",
                ops::quote(&format!("exec 3<>/dev/tcp/127.0.0.1/{port}"))
            );
            let said = match inside::probe(carrier, &script).await {
                Ok(yes) => BoxAnswer::Listens { yes },
                Err(e) => refused(format!("cannot probe the port: {e}")),
            };
            return answer(&mut stream, said).await;
        }
        match inside::start_forward(carrier, &self.provider.forward_script(*port)) {
            Ok(child) => {
                answer(&mut stream, BoxAnswer::Streaming).await;
                inside::bridge(child, stream, self.provider.ender(&slug, &mark)).await;
            }
            Err(e) => answer(&mut stream, refused(format!("cannot reach the port: {e}"))).await,
        }
    }
}
