// The log store's streams (#467): files appended to across runs, at
// $SHEEPDOG_STATE_DIR/logs/streams/<name>; dir 0700, files 0600. Rotated by
// rename, so a writer holding one open goes on in <name>.1. launchd opens
// StandardOutPath each time it starts the job: it appends to one that is
// there and makes a missing one by the umask (0644).

use crate::config;
use std::fs;
use std::io;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const LAUNCHD: &str = "launchd.log";
pub const STEPS: &str = "steps.log";

pub fn dir() -> PathBuf {
    Path::new(&config::state_dir()).join("logs/streams")
}

/// <name> in <dir>, opened to append; dir and file made private.
pub fn open(dir: &Path, name: &str) -> io::Result<fs::File> {
    super::private_dir(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    let f = fs::OpenOptions::new().create(true).append(true).mode(0o600).open(dir.join(name))?;
    f.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(f)
}

/// <name> in <dir> rotated once it's <bytes> or more (0: never), keeping
/// <files> (0: every one); then <name> is there, empty or not, at 0600.
pub fn rotate(dir: &Path, name: &str, bytes: u64, files: u64) -> io::Result<()> {
    let size = open(dir, name)?.metadata()?.len();
    if bytes == 0 || size < bytes {
        return Ok(());
    }
    let nth = |k: u64| dir.join(format!("{name}.{k}"));
    // The highest kept: renaming onto it drops the one there.
    let top = if files == 0 { (1..).find(|k| !nth(*k).exists()).unwrap_or(1) } else { files - 1 };
    if top == 0 {
        fs::remove_file(dir.join(name))?;
    } else {
        for k in (1..top).rev().filter(|k| nth(*k).exists()) {
            fs::rename(nth(k), nth(k + 1))?;
        }
        fs::rename(dir.join(name), nth(1))?;
    }
    open(dir, name).map(drop)
}

/// The job's pass, after its last write: each stream rotated by
/// settings.logStream*, each key's text read by <get>.
pub fn rotate_all(get: impl Fn(&str) -> String) -> io::Result<()> {
    let n = |k: &str, d: u64| get(&format!("settings.{k}")).parse().unwrap_or(d);
    let (bytes, files) = (n("logStreamBytes", 10 << 20), n("logStreamFiles", 3));
    [LAUNCHD, STEPS].iter().try_for_each(|name| rotate(&dir(), name, bytes, files))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn tmp(name: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("sd-streams-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        Dir(d)
    }

    fn mode(p: &Path) -> u32 {
        fs::metadata(p).unwrap().mode() & 0o777
    }

    fn read(p: &Path) -> String {
        fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    #[test]
    fn open_makes_the_dir_0700_and_the_file_0600_and_appends() {
        let d = tmp("open");
        let streams = d.0.join("logs/streams");
        io::Write::write_all(&mut open(&streams, STEPS).unwrap(), b"one\n").unwrap();
        io::Write::write_all(&mut open(&streams, STEPS).unwrap(), b"two\n").unwrap();
        assert_eq!(read(&streams.join(STEPS)), "one\ntwo\n");
        assert_eq!(mode(&streams.join(STEPS)), 0o600);
        assert_eq!(mode(&streams), 0o700);
        assert_eq!(mode(&d.0.join("logs")), 0o700);
    }

    #[test]
    fn under_the_size_nothing_moves() {
        let d = tmp("under");
        fs::create_dir_all(&d.0).unwrap();
        fs::write(d.0.join(STEPS), "123456789").unwrap();
        rotate(&d.0, STEPS, 10, 3).unwrap();
        assert_eq!(read(&d.0.join(STEPS)), "123456789");
        assert!(!d.0.join("steps.log.1").exists());
    }

    #[test]
    fn at_the_size_each_moves_up_one_and_the_oldest_past_the_count_goes() {
        let d = tmp("at");
        fs::create_dir_all(&d.0).unwrap();
        fs::write(d.0.join(STEPS), "live-0123").unwrap();
        fs::write(d.0.join("steps.log.1"), "one").unwrap();
        fs::write(d.0.join("steps.log.2"), "two").unwrap();
        rotate(&d.0, STEPS, 9, 3).unwrap();
        assert_eq!(read(&d.0.join(STEPS)), "");
        assert_eq!(read(&d.0.join("steps.log.1")), "live-0123");
        assert_eq!(read(&d.0.join("steps.log.2")), "one");
        assert!(!d.0.join("steps.log.3").exists(), "three kept, the live one included");
    }

    #[test]
    fn a_count_of_one_keeps_only_the_new_file_and_zero_keeps_every_one() {
        let d = tmp("count");
        fs::create_dir_all(&d.0).unwrap();
        fs::write(d.0.join(STEPS), "old").unwrap();
        rotate(&d.0, STEPS, 1, 1).unwrap();
        assert_eq!(read(&d.0.join(STEPS)), "");
        assert!(!d.0.join("steps.log.1").exists());

        for n in 0..4 {
            fs::write(d.0.join(STEPS), format!("run{n}")).unwrap();
            rotate(&d.0, STEPS, 1, 0).unwrap();
        }
        let kept: Vec<String> = (1..=4).map(|k| read(&d.0.join(format!("steps.log.{k}")))).collect();
        assert_eq!(kept, ["run3", "run2", "run1", "run0"]);
    }

    #[test]
    fn a_size_of_zero_never_rotates() {
        let d = tmp("zero");
        fs::create_dir_all(&d.0).unwrap();
        fs::write(d.0.join(STEPS), "x".repeat(4096)).unwrap();
        rotate(&d.0, STEPS, 0, 3).unwrap();
        assert_eq!(read(&d.0.join(STEPS)).len(), 4096);
    }

    #[test]
    fn after_a_rotate_the_files_are_0600_and_the_dir_0700_whatever_made_them() {
        let d = tmp("modes");
        fs::create_dir_all(&d.0).unwrap();
        fs::set_permissions(&d.0, fs::Permissions::from_mode(0o755)).unwrap();
        // As launchd makes a missing StandardOutPath: by the umask.
        fs::write(d.0.join(LAUNCHD), "launchd's").unwrap();
        fs::set_permissions(d.0.join(LAUNCHD), fs::Permissions::from_mode(0o644)).unwrap();
        rotate(&d.0, LAUNCHD, 1 << 20, 3).unwrap();
        assert_eq!(mode(&d.0.join(LAUNCHD)), 0o600, "under the size, still made private");
        rotate(&d.0, LAUNCHD, 1, 3).unwrap();
        assert_eq!(mode(&d.0.join(LAUNCHD)), 0o600, "the new one");
        assert_eq!(mode(&d.0.join("launchd.log.1")), 0o600);
        assert_eq!(mode(&d.0), 0o700);

        let fresh = tmp("fresh");
        rotate(&fresh.0, STEPS, 10, 3).unwrap();
        assert_eq!(mode(&fresh.0.join(STEPS)), 0o600, "a missing stream is made, empty");
        assert_eq!(read(&fresh.0.join(STEPS)), "");
    }
}
