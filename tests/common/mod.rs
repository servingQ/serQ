#![allow(dead_code)]

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

pub const PROGRAM: &str = "use \"std/args\"; let rate = args.number(\"rate\", 1);\nstage svc : fifo;\nworkload { arrive batch(1); session { request; end; } }\nserver { run svc (cost(svc, rate)); }\n\n";

pub struct Fixture(pub PathBuf);

impl Fixture {
    pub fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "serq-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }

    pub fn write(&self, file: &str, contents: &str) {
        let path = self.0.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    pub fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_serq"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn failure(output: &Output, code: i32, fragments: &[&str]) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(code), "{stderr}");
    assert!(output.stdout.is_empty(), "{:?}", output.stdout);
    for fragment in fragments {
        assert!(stderr.contains(fragment), "missing {fragment:?}: {stderr}");
    }
}

/// Existing semantic fixtures describe a main body; complete example files
/// already contain their entry point. The entrypoint tests use the public API
/// directly, so this builder cannot make an implicit program pass those checks.
pub fn main_source(body: &str) -> String {
    if body.contains("fn main()") {
        body.to_string()
    } else {
        format!("fn main() {{ {body}\n}}")
    }
}

/// Execution conditions belong to the test, separately from its model text.
pub fn horizon(horizon: f64) -> serq::Overrides {
    serq::Overrides {
        horizon: Some(horizon),
        ..Default::default()
    }
}
