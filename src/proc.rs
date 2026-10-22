// Running a command.

use std::io::{ErrorKind, Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Debug, Default)]
pub struct Out {
    pub ok: bool,
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    /// with_timeout gave up: code is 124, and the output ends with
    /// "timed out after <secs>s".
    pub timed_out: bool,
}

fn not_found(prog: &str) -> Out {
    Out { code: 127, stderr: format!("{prog}: not found\n"), ..Out::default() }
}

pub fn run(prog: &str, args: &[&str], dir: Option<&str>, input: Option<&[u8]>) -> Out {
    let mut c = Command::new(prog);
    c.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    c.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() });
    if let Some(d) = dir {
        c.current_dir(d);
    }
    let Ok(mut child) = c.spawn() else { return not_found(prog) };
    if let Some(i) = input {
        let mut stdin = child.stdin.take().expect("piped");
        let i = i.to_vec();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&i);
        });
    }
    let o = child.wait_with_output().expect("wait on a spawned child");
    Out {
        ok: o.status.success(),
        code: o.status.code().unwrap_or(128),
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        timed_out: false,
    }
}

fn command(prog: &str, args: &[&str], dir: Option<&str>) -> Command {
    let mut c = Command::new(prog);
    c.args(args);
    if let Some(d) = dir {
        c.current_dir(d);
    }
    c
}

pub fn with_timeout(secs: u64, prog: &str, args: &[&str], dir: Option<&str>) -> Out {
    timeout(secs, command(prog, args, dir), false)
}

/// with_timeout with stderr in stdout, interleaved as 2>&1 would have it.
pub fn with_timeout_merged(secs: u64, prog: &str, args: &[&str], dir: Option<&str>) -> Out {
    timeout(secs, command(prog, args, dir), true)
}

type Buf = Arc<Mutex<Vec<u8>>>;

/// Into a buffer readable before EOF: a process that ignores TERM can hold
/// the pipe open.
fn drain(mut r: impl Read + Send + 'static) -> (Buf, JoinHandle<()>) {
    let buf = Buf::default();
    let b = Arc::clone(&buf);
    let h = std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match r.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => b.lock().expect("no reader panics").extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    });
    (buf, h)
}

fn text(b: &Buf) -> String {
    String::from_utf8_lossy(&b.lock().expect("no reader panics")).into_owned()
}

/// The shell's $?: a command killed by a signal is 128 + the signal.
fn code(st: ExitStatus) -> i32 {
    st.code().unwrap_or_else(|| 128 + st.signal().unwrap_or(0))
}

// In its own group, a Ctrl-C or a kill of sd doesn't reach timeout's command,
// so as lib/sheepdog.sh's with_timeout does, a HUP, INT or TERM is passed on
// to the group, and waited for, before it ends sd too (#170). libc's, which
// std links already: no crate.
extern "C" {
    fn signal(sig: i32, handler: usize) -> usize;
    fn raise(sig: i32) -> i32;
}
const SIG_DFL: usize = 0;
const SIG_IGN: usize = 1;
/// HUP, INT and TERM: the same numbers on macOS and Linux.
const PASSED_ON: [i32; 3] = [1, 2, 15];

/// How many timeouts are running, and the signal sd got while one was.
static RUNNING: AtomicUsize = AtomicUsize::new(0);
static CAUGHT: AtomicI32 = AtomicI32::new(0);

fn die_of(sig: i32) {
    unsafe {
        signal(sig, SIG_DFL);
        raise(sig);
    }
}

/// Only async-signal-safe calls: an atomic, signal, raise.
extern "C" fn caught(sig: i32) {
    if RUNNING.load(Ordering::SeqCst) == 0 {
        die_of(sig);
    } else {
        CAUGHT.store(sig, Ordering::SeqCst);
    }
}

/// Once per process. A signal sd started out ignoring (nohup's HUP, a
/// background job's INT) stays ignored.
fn pass_signals_on() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        for sig in PASSED_ON {
            unsafe {
                if signal(sig, caught as extern "C" fn(i32) as usize) == SIG_IGN {
                    signal(sig, SIG_IGN);
                }
            }
        }
    });
}

/// <c> in its own process group, stdin from /dev/null. At the deadline the
/// whole group gets TERM (an agent's subprocesses too), KILL a second later
/// if the command is still running, and it's Out's timed_out: status 124,
/// "timed out after <secs>s" on stderr (or on stdout, merged), after what the
/// command printed till then. A HUP, INT or TERM to sd ends the group the
/// same way, then sd.
pub fn timeout(secs: u64, c: Command, merged: bool) -> Out {
    pass_signals_on();
    RUNNING.fetch_add(1, Ordering::SeqCst);
    let o = timeout_in_group(secs, c, merged);
    let last = RUNNING.fetch_sub(1, Ordering::SeqCst) == 1;
    let sig = CAUGHT.load(Ordering::SeqCst);
    if last && sig != 0 {
        die_of(sig);
    }
    o
}

fn timeout_in_group(secs: u64, mut c: Command, merged: bool) -> Out {
    let prog = c.get_program().to_string_lossy().into_owned();
    c.stdin(Stdio::null()).process_group(0);
    let child = if merged {
        let pipe = std::io::pipe().and_then(|(r, w)| Ok((r, w.try_clone()?, w)));
        let Ok((r, w1, w2)) = pipe else { return not_found(&prog) };
        c.stdout(w1).stderr(w2);
        c.spawn().map(|ch| (ch, Some(r)))
    } else {
        c.stdout(Stdio::piped()).stderr(Stdio::piped());
        c.spawn().map(|ch| (ch, None))
    };
    // The command's copies of the merged pipe's write ends close with it.
    drop(c);
    let Ok((mut child, merged_r)) = child else { return not_found(&prog) };
    let (out, out_h) = match merged_r {
        Some(r) => drain(r),
        None => drain(child.stdout.take().expect("piped")),
    };
    let (err, err_h) = child.stderr.take().map(drain).unzip();
    let readers: Vec<JoinHandle<()>> = std::iter::once(out_h).chain(err_h).collect();

    // As $( ) does, wait for what it started to stop writing too, but only
    // till the deadline: a grandchild holding the pipe doesn't hold us.
    let deadline = Instant::now() + Duration::from_secs(secs);
    let finished = |rs: &[JoinHandle<()>]| rs.iter().all(JoinHandle::is_finished);
    let mut status = None;
    let signalled = || CAUGHT.load(Ordering::SeqCst) != 0;
    while Instant::now() < deadline && !signalled() {
        status = status.or_else(|| child.try_wait().ok().flatten());
        if status.is_some() && finished(&readers) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if status.is_some() && finished(&readers) && !signalled() {
        for h in readers {
            let _ = h.join();
        }
    } else {
        // The bash with_timeout didn't wait at all; this waits a second for
        // the group to stop, then KILLs it if anything in it ignored TERM.
        // The leader exiting doesn't mean the group is gone: a grandchild
        // that ignored TERM still holds the pipes.
        let group = format!("-{}", child.id());
        // "--" so kill can't read the negative pid as an option.
        let kill = |sig: &str| Command::new("kill").args([sig, "--", &group]).status();
        let _ = kill("-TERM");
        let grace = Instant::now() + Duration::from_secs(1);
        let mut exited = || matches!(child.try_wait(), Ok(Some(_)));
        while !(exited() && finished(&readers)) && Instant::now() < grace {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !(exited() && finished(&readers)) {
            let _ = kill("-KILL");
            let _ = child.wait();
        }
    }
    let mut o = Out {
        ok: status.is_some_and(|s| s.success()),
        code: status.map_or(124, code),
        stdout: text(&out),
        stderr: err.as_ref().map(text).unwrap_or_default(),
        timed_out: status.is_none(),
    };
    if o.timed_out {
        let to = if merged { &mut o.stdout } else { &mut o.stderr };
        to.push_str(&format!("timed out after {secs}s\n"));
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_timeout_returns_stdout_stderr_and_status() {
        let o = with_timeout(5, "sh", &["-c", "echo out; echo err >&2; exit 3"], None);
        assert_eq!((o.ok, o.code, o.timed_out), (false, 3, false));
        assert_eq!(o.stdout, "out\n");
        assert_eq!(o.stderr, "err\n");
        let o = with_timeout(5, "pwd", &[], Some("/"));
        assert!(o.ok);
        assert_eq!(o.stdout, "/\n");
    }

    #[test]
    fn with_timeout_merged_interleaves_stderr_into_stdout() {
        let o = with_timeout_merged(5, "sh", &["-c", "echo 1; echo 2 >&2; echo 3"], None);
        assert!(o.ok);
        assert_eq!(o.stdout, "1\n2\n3\n");
        assert_eq!(o.stderr, "");
    }

    #[test]
    fn with_timeout_kills_the_whole_group_and_says_so_as_the_bash_did() {
        let dir = std::env::temp_dir().join(format!("sd-proc-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let late = dir.join("late");
        // A grandchild in the background: only a kill of the group stops it
        // writing. (Checked by what it does, not kill -0: an orphan nobody
        // reaps stays a zombie.)
        let script = format!("echo early; (sleep 4; echo late > {}) & wait", late.display());
        let start = Instant::now();
        let o = with_timeout(1, "sh", &["-c", &script], None);
        assert!(start.elapsed() < Duration::from_secs(10));
        assert_eq!((o.ok, o.code, o.timed_out), (false, 124, true));
        assert_eq!(o.stdout, "early\n");
        assert_eq!(o.stderr, "timed out after 1s\n");
        std::thread::sleep(Duration::from_secs(5).saturating_sub(start.elapsed()));
        assert!(!late.exists(), "the grandchild outlived the timeout");
        let o = with_timeout_merged(1, "sh", &["-c", "echo early >&2; sleep 30"], None);
        assert_eq!(o.stdout, "early\ntimed out after 1s\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_timeout_doesnt_wait_on_a_command_that_ignores_term() {
        let start = Instant::now();
        let o = with_timeout(1, "sh", &["-c", "trap '' TERM; echo hi; sleep 12; echo after"], None);
        assert!(start.elapsed() < Duration::from_secs(8), "waited {:?}", start.elapsed());
        assert_eq!((o.ok, o.code, o.timed_out), (false, 124, true));
        assert_eq!(o.stdout, "hi\n");
        assert_eq!(o.stderr, "timed out after 1s\n");
    }

    #[test]
    fn with_timeout_kills_a_grandchild_that_ignores_term_after_the_leader_exits() {
        let dir = std::env::temp_dir().join(format!("sd-proc-test-term-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let late = dir.join("late");
        // The leader dies on TERM; the grandchild ignores it and holds the pipe.
        let script = format!("(trap '' TERM; sleep 8; echo late > {}) & wait", late.display());
        let start = Instant::now();
        let o = with_timeout(1, "sh", &["-c", &script], None);
        assert!(start.elapsed() < Duration::from_secs(6), "waited {:?}", start.elapsed());
        assert_eq!((o.code, o.timed_out), (124, true));
        std::thread::sleep(Duration::from_secs(9).saturating_sub(start.elapsed()));
        assert!(!late.exists(), "the grandchild outlived the timeout");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_timeout_doesnt_wait_past_the_deadline_on_a_grandchild_holding_the_pipe() {
        let start = Instant::now();
        let o = with_timeout(1, "sh", &["-c", "echo hi; sleep 30 & exit 0"], None);
        assert!(start.elapsed() < Duration::from_secs(10), "waited {:?}", start.elapsed());
        // The command itself finished in time: its status, not a timeout.
        assert_eq!((o.ok, o.code, o.timed_out), (true, 0, false));
        assert_eq!(o.stdout, "hi\n");
    }

    #[test]
    fn with_timeout_on_a_missing_program_is_127() {
        let o = with_timeout(5, "no-such-program-sd", &[], None);
        assert_eq!((o.ok, o.code, o.timed_out), (false, 127, false));
    }

    #[test]
    fn a_command_killed_by_a_signal_is_128_plus_it() {
        let o = with_timeout(5, "sh", &["-c", "kill -TERM $$"], None);
        assert_eq!(o.code, 143);
    }
}
