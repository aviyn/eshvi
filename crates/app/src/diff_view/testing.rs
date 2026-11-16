//! Repositories for the tests, in a folder of their own.

use std::path::PathBuf;
use std::process::Command;

/// A repository in a folder of its own, with the commits' files.
pub struct Repo {
    pub dir: PathBuf,
}

impl Repo {
    pub fn new(name: &str) -> Repo {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("eshvi-diff-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = Repo { dir };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.email", "t@example.com"]);
        repo.git(&["config", "user.name", "T"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo
    }

    pub fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git").arg("-C").arg(&self.dir).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    pub fn write(&self, path: &str, text: &str) {
        let full = self.dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }

    pub fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

