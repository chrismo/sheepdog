// The black-box harness tests/<verb>.rs share (#199): the built binary, run
// as sd-<verb> through a link, in a temp dir that holds git's global config,
// HOME, the global sheepdog config and fakebin/ (first on PATH). Each suite
// builds its own repos in it: tests/config.rs one repo, tests/state.rs a bare
// origin and two clones.

#![allow(dead_code)] // each suite uses its own part

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::RwLock;

/// Held for writing while a fake is written, and for reading while a child is
/// spawned: a spawn on another thread would carry the fake's open fd into its
/// child, and running the fake then fails with ETXTBSY (Linux).
static SPAWN: RwLock<()> = RwLock::new(());

pub fn spawn(c: &mut Command) -> Child {
    let _g = SPAWN.read().unwrap();
    c.spawn().unwrap_or_else(|e| panic!("{c:?}: {e}"))
}

/// The command's output, stdin fed from <input> or else empty.
pub fn output(c: &mut Command, input: Option<&[u8]>) -> Output {
    c.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = spawn(c);
    if let Some(i) = input {
        child.stdin.take().unwrap().write_all(i).unwrap();
    }
    child.wait_with_output().unwrap()
}

/// One run, as bats `run` sees it: `output` is stdout and stderr together,
/// or stdout alone from `run_sep` (bats' --separate-stderr), trailing
/// newlines stripped.
pub struct Run {
    pub status: i32,
    pub output: String,
    pub stderr: String,
}

impl Run {
    /// bats' `lines`: the output's non-empty lines.
    pub fn lines(&self) -> Vec<&str> {
        self.output.split('\n').filter(|l| !l.is_empty()).collect()
    }

    /// `sed -n <n>p`: the output's nth line, 1-based, or "".
    pub fn line(&self, n: usize) -> &str {
        self.output.split('\n').nth(n - 1).unwrap_or("")
    }
}

pub fn chomp(s: &str) -> String {
    s.trim_end_matches('\n').to_string()
}

/// jq <args> over <input>, which must succeed; its stdout, chomped.
pub fn jq(args: &[&str], input: &str) -> String {
    let out = output(Command::new("jq").args(args), Some(input.as_bytes()));
    assert!(
        out.status.success(),
        "jq {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    chomp(&String::from_utf8_lossy(&out.stdout))
}

/// A temp dir holding bin/sd-<verb>, a link to the binary under test; the
/// global config file's path (global.json, not yet written); home/ for HOME;
/// fakebin/, first on PATH; and git's global config.
pub struct Sd {
    root: PathBuf,
    pub tmp: PathBuf,
    pub cwd: PathBuf,
    pub bin: PathBuf,
    pub config: Option<PathBuf>,
    pub xdg: Option<PathBuf>,
    /// Set for every command, as a bats setup's exports are.
    pub env: Vec<(String, String)>,
}

impl Drop for Sd {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Sd {
    pub fn for_verb(verb: &str) -> Sd {
        static N: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "sd-{verb}-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let tmp = fs::canonicalize(&root).unwrap();
        for d in ["bin", "fakebin", "home"] {
            fs::create_dir(tmp.join(d)).unwrap();
        }
        fs::write(
            tmp.join("gitconfig-global"),
            "[user]\n\temail = t@example.com\n\tname = Tester\n[init]\n\tdefaultBranch = main\n",
        )
        .unwrap();
        let sd = Sd {
            root,
            cwd: tmp.clone(),
            config: Some(tmp.join("global.json")),
            xdg: None,
            bin: tmp.join(format!("bin/sd-{verb}")),
            env: Vec::new(),
            tmp,
        };
        sd.link(verb);
        sd
    }

    /// <path> in the temp dir, as a string.
    pub fn path(&self, path: &str) -> String {
        self.tmp.join(path).display().to_string()
    }

    /// cd <path>, relative to the temp dir.
    pub fn cd(&mut self, path: &str) {
        self.cwd = self.tmp.join(path);
    }

    /// bin/sd-<verb>, a link to the binary, made on first use: another verb
    /// a test runs, as the bats suites ran libexec/sd-config.
    pub fn link(&self, verb: &str) -> PathBuf {
        let l = self.tmp.join(format!("bin/sd-{verb}"));
        if !l.exists() {
            std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_sd"), &l).unwrap();
        }
        l
    }

    /// fakebin/<name> := <script>, executable.
    pub fn fake(&self, name: &str, script: &str) {
        let _g = SPAWN.write().unwrap();
        let f = self.tmp.join("fakebin").join(name);
        fs::write(&f, script).unwrap();
        fs::set_permissions(&f, fs::Permissions::from_mode(0o755)).unwrap();
    }

    pub fn command(&self, program: &Path) -> Command {
        let mut c = Command::new(program);
        let path = std::env::var("PATH").unwrap_or_default();
        c.current_dir(&self.cwd)
            .env(
                "PATH",
                format!("{}:{path}", self.tmp.join("fakebin").display()),
            )
            .env("HOME", self.tmp.join("home"))
            .env("GIT_CONFIG_GLOBAL", self.tmp.join("gitconfig-global"))
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env_remove("SHEEPDOG_CONFIG")
            .env_remove("XDG_CONFIG_HOME");
        for v in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_COMMON_DIR",
            "GIT_INDEX_FILE",
        ] {
            c.env_remove(v);
        }
        if let Some(p) = &self.config {
            c.env("SHEEPDOG_CONFIG", p);
        }
        if let Some(p) = &self.xdg {
            c.env("XDG_CONFIG_HOME", p);
        }
        for (k, v) in &self.env {
            c.env(k, v);
        }
        c
    }

    fn git_output(&self, env: &[(&str, &str)], input: Option<&str>, args: &[&str]) -> Output {
        let mut c = self.command(Path::new("git"));
        c.args(args).envs(env.iter().copied());
        output(&mut c, input.map(str::as_bytes))
    }

    pub fn git_status(&self, args: &[&str]) -> i32 {
        self.git_output(&[], None, args).status.code().unwrap_or(-1)
    }

    /// git <args> with <env> and <input> on stdin, which must succeed; its
    /// stdout, chomped.
    pub fn git_with(&self, env: &[(&str, &str)], input: Option<&str>, args: &[&str]) -> String {
        let out = self.git_output(env, input, args);
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        chomp(&String::from_utf8_lossy(&out.stdout))
    }

    /// git <args>, which must succeed; its stdout, chomped.
    pub fn git(&self, args: &[&str]) -> String {
        self.git_with(&[], None, args)
    }

    /// git <args>'s stdout, chomped, or None when it fails.
    pub fn git_try(&self, args: &[&str]) -> Option<String> {
        let out = self.git_output(&[], None, args);
        out.status
            .success()
            .then(|| chomp(&String::from_utf8_lossy(&out.stdout)))
    }

    /// git -C <dir> <args>, <dir> relative to the temp dir.
    pub fn git_in(&self, dir: &str, args: &[&str]) -> String {
        let d = self.path(dir);
        self.git(&[&["-C", &d], args].concat())
    }

    /// A bare origin.git, cloned to each of <clones>.
    pub fn origin(&self, clones: &[&str]) {
        self.git(&["init", "-q", "--bare", &self.path("origin.git")]);
        for c in clones {
            self.git(&["clone", "-q", &self.path("origin.git"), &self.path(c)]);
        }
    }

    /// <program> <args> with <env>: stdout and stderr interleaved on one
    /// pipe, as bats `run` has them.
    pub fn exec(&self, program: &Path, env: &[(&str, &str)], args: &[&str]) -> Run {
        let (mut r, w) = std::io::pipe().unwrap();
        let mut c = self.command(program);
        c.args(args)
            .envs(env.iter().copied())
            .stdin(Stdio::null())
            .stdout(w.try_clone().unwrap())
            .stderr(w);
        let mut child = spawn(&mut c);
        drop(c);
        let mut buf = Vec::new();
        r.read_to_end(&mut buf).unwrap();
        let status = child.wait().unwrap().code().unwrap_or(-1);
        Run {
            status,
            output: chomp(&String::from_utf8_lossy(&buf)),
            stderr: String::new(),
        }
    }

    /// bats `run`: stdout and stderr interleaved on one pipe.
    pub fn run(&self, args: &[&str]) -> Run {
        self.exec(&self.bin, &[], args)
    }

    /// `VAR=value run`: <env> for this run only.
    pub fn run_env(&self, env: &[(&str, &str)], args: &[&str]) -> Run {
        self.exec(&self.bin, env, args)
    }

    fn bin_output(&self, args: &[&str]) -> Output {
        output(self.command(&self.bin).args(args), None)
    }

    /// bats `run --separate-stderr`.
    pub fn run_sep(&self, args: &[&str]) -> Run {
        let out = self.bin_output(args);
        Run {
            status: out.status.code().unwrap_or(-1),
            output: chomp(&String::from_utf8_lossy(&out.stdout)),
            stderr: chomp(&String::from_utf8_lossy(&out.stderr)),
        }
    }

    /// stdout as written, whatever the status.
    pub fn stdout(&self, args: &[&str]) -> String {
        String::from_utf8_lossy(&self.bin_output(args).stdout).into_owned()
    }

    /// `$(sd-<verb> <args>)`.
    pub fn out(&self, args: &[&str]) -> String {
        chomp(&self.stdout(args))
    }

    /// A bare command line in a bats test, which must succeed; its stdout.
    pub fn ok(&self, args: &[&str]) -> String {
        self.prog_ok(&self.bin, args)
    }

    /// The same for another verb: `"$SD_CONFIG" set ...` in a bats test.
    pub fn verb_ok(&self, verb: &str, args: &[&str]) -> String {
        self.prog_ok(&self.link(verb), args)
    }

    fn prog_ok(&self, program: &Path, args: &[&str]) -> String {
        let out = output(self.command(program).args(args), None);
        assert!(
            out.status.success(),
            "{} {args:?}: {}",
            program.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    pub fn cfg(&self) -> &Path {
        self.config.as_deref().unwrap()
    }

    pub fn write_global(&self, s: &str) {
        fs::write(self.cfg(), format!("{s}\n")).unwrap();
    }

    /// cwd/<name> := <content>
    pub fn write(&self, name: &str, content: &str) {
        fs::write(self.cwd.join(name), content).unwrap();
    }
}
