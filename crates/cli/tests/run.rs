use std::process::Command;

#[test]
fn reports_defaults_and_rejects_invalid_arguments() {
    let directory = tempfile::tempdir().unwrap();
    for (args, expected) in [
        (
            vec!["--help"],
            "apps run [entry] [--watch|-W] [--devtools|-D]",
        ),
        (vec!["run", "--help"], "Watching is off by default"),
        (vec!["build", "--help"], "apps build [entry]"),
        (vec!["run"], "No entry found"),
        (vec!["build"], "No entry found"),
        (vec!["dev"], "Unknown command: dev"),
        (vec!["run", "--invalid"], "Unknown option: --invalid"),
        (
            vec!["run", "--out-dir", "output"],
            "Unknown option: --out-dir",
        ),
        (vec!["build", "--watch"], "Unknown option: --watch"),
        (vec!["build", "-D"], "Unknown option: -D"),
        (vec!["check"], "Unknown command: check"),
        (vec!["check", "--help"], "Unknown command: check"),
        (
            vec!["build", "one.js", "two.js"],
            "Unexpected argument: two.js",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_apps"))
            .args(&args)
            .current_dir(directory.path())
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(text.contains(expected), "{args:?}: {text}");
        assert!(!text.contains("apps check"));
        assert_eq!(
            output.status.success(),
            args.contains(&"--help") && args[0] != "check"
        );
    }
}

#[cfg(unix)]
mod process {
    use super::*;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        process::{Child, ExitStatus, Stdio},
        time::{Duration, Instant},
    };

    struct App {
        directory: tempfile::TempDir,
        root: PathBuf,
        child: Option<Child>,
    }

    impl App {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().canonicalize().unwrap().join("project");
            fs::create_dir(&root).unwrap();
            let runtime = directory.path().join("runtime");
            fs::write(&runtime, "#!/bin/sh\ncat > \"$APPS_RUN_BUNDLE\"\nprintf '%s\\n' \"$*\" >> \"$APPS_RUN_LOG\"\nexit \"${APPS_RUN_EXIT:-0}\"\n").unwrap();
            fs::set_permissions(runtime, fs::Permissions::from_mode(0o755)).unwrap();
            Self {
                directory,
                root,
                child: None,
            }
        }

        fn command(&self) -> Command {
            let mut command = Command::new(env!("CARGO_BIN_EXE_apps"));
            command
                .arg("run")
                .current_dir(&self.root)
                .env("APPS_RUNTIME", self.directory.path().join("runtime"))
                .env("APPS_RUN_BUNDLE", self.directory.path().join("bundle.json"))
                .env("APPS_RUN_LOG", self.directory.path().join("launches"));
            command
        }

        fn launches(&self) -> String {
            fs::read_to_string(self.directory.path().join("launches")).unwrap_or_default()
        }

        fn wait(&mut self, count: usize) {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let launches = self.launches().lines().count();
                if launches == count {
                    break;
                }
                assert!(launches < count, "Unexpected launch: {}", self.launches());
                assert!(
                    self.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                    "Watcher exited"
                );
                assert!(
                    Instant::now() < deadline,
                    "Expected {count} launches, got {launches}"
                );
                std::thread::sleep(Duration::from_millis(25));
            }
        }

        fn stop(&mut self) -> Option<ExitStatus> {
            let child = self.child.as_mut()?;
            if let Some(status) = child.try_wait().unwrap() {
                return Some(status);
            }
            let _ = Command::new("kill")
                .args(["-INT", &child.id().to_string()])
                .output();
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if let Some(status) = child.try_wait().unwrap() {
                    return Some(status);
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            let _ = child.kill();
            child.wait().ok()
        }
    }

    impl Drop for App {
        fn drop(&mut self) {
            self.stop();
        }
    }

    #[test]
    fn resolves_default_entries_and_forwards_devtools_and_exit_status() {
        let app = App::new();
        fs::write(
            app.root.join("index.ts"),
            "const n: number = 42; console.log(n);",
        )
        .unwrap();
        fs::write(app.root.join("index.js"), "invalid javascript !!!").unwrap();
        let output = app.command().output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("watching"));
        let bundle: apps_runtime::bundle::Bundle =
            serde_json::from_slice(&fs::read(app.directory.path().join("bundle.json")).unwrap())
                .unwrap();
        assert!(bundle.files["app.js"].contains("42"));
        assert!(bundle.files["app.js"].contains("\"map\":"));
        assert!(!bundle.packaged);
        assert!(!app.root.join(".apps").exists());
        assert!(!app.root.join("dist").exists());
        fs::remove_file(app.root.join("index.ts")).unwrap();
        fs::write(app.root.join("index.js"), "console.log(42);").unwrap();
        for args in [
            vec!["-D"],
            vec!["--devtools", "index.js"],
            vec!["index.js", "-D"],
            vec!["index.js", "a", "--flag", "--", "-W"],
        ] {
            let output = app
                .command()
                .args(args)
                .env("APPS_RUN_EXIT", "7")
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(7),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let (ts, js) = (app.root.join("index.ts"), app.root.join("index.js"));
        let (ts, js) = (ts.display(), js.display());
        assert_eq!(
            app.launches(),
            format!(
                "--bundle-stdin -- {ts}\n--bundle-stdin --devtools -- {js}\n--bundle-stdin --devtools -- {js}\n--bundle-stdin --devtools -- {js}\n--bundle-stdin -- {js} a --flag -W\n"
            )
        );
    }

    #[test]
    fn streams_large_bundles_from_read_only_projects() {
        let app = App::new();
        let value = "large bundle 🫖".repeat(16384);
        fs::write(
            app.root.join("index.js"),
            format!("console.log({value:?});"),
        )
        .unwrap();
        fs::create_dir(app.root.join("public")).unwrap();
        fs::write(app.root.join("public/asset.txt"), "asset").unwrap();
        fs::set_permissions(&app.root, fs::Permissions::from_mode(0o555)).unwrap();
        let output = app.command().env("APPS_RUNTIME", "../runtime").output();
        fs::set_permissions(&app.root, fs::Permissions::from_mode(0o755)).unwrap();
        let output = output.unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bundle: apps_runtime::bundle::Bundle =
            serde_json::from_slice(&fs::read(app.directory.path().join("bundle.json")).unwrap())
                .unwrap();
        let app_js = &bundle.files["app.js"];
        assert!(app_js.is_ascii());
        let module: serde_json::Value = serde_json::from_str(
            app_js
                .trim_end()
                .strip_prefix("$appsDefine(")
                .and_then(|source| source.strip_suffix(");"))
                .unwrap(),
        )
        .unwrap();
        assert!(
            module["map"]["sourcesContent"][0]
                .as_str()
                .unwrap()
                .contains(&value)
        );
        assert_eq!(fs::read_dir(&app.root).unwrap().count(), 2);
    }

    #[test]
    fn watches_imports_atomic_saves_and_dependency_changes() {
        let mut app = App::new();
        let shared = app.directory.path().join("shared.ts");
        fs::write(&shared, "export const value = 1;").unwrap();
        let package = app.root.join("node_modules/dep");
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join("package.json"),
            r#"{"main":"index.js","type":"module"}"#,
        )
        .unwrap();
        fs::write(package.join("index.js"), "export const other = 2;").unwrap();
        let entry = app.root.join("index.ts");
        let source = "import {value} from '../shared.ts'; import {other} from 'dep'; console.log(value, other);";
        fs::write(&entry, source).unwrap();
        app.child = Some(
            app.command()
                .args(["-W", "--devtools"])
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
        app.wait(1);
        fs::write(app.root.join("unused.ts"), "console.log('unused');").unwrap();
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(app.launches().lines().count(), 1);
        let saved = shared.with_extension("tmp");
        fs::write(&saved, "export const value = 3;").unwrap();
        fs::rename(saved, &shared).unwrap();
        app.wait(2);
        fs::write(package.join("index.js"), "export const other = 4;").unwrap();
        app.wait(3);
        fs::write(app.root.join("next.ts"), "export const next = 5;").unwrap();
        fs::write(&entry, "import {next} from './next.ts'; console.log(next);").unwrap();
        app.wait(4);
        fs::write(&shared, "export const value = 6;").unwrap();
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(app.launches().lines().count(), 4);
        fs::write(app.root.join("next.ts"), "export const next = 7;").unwrap();
        app.wait(5);
        fs::remove_file(app.root.join("next.ts")).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(app.launches().lines().count(), 5);
        fs::write(app.root.join("next.ts"), "export const next = 8;").unwrap();
        app.wait(6);
        assert!(
            app.launches()
                .lines()
                .all(|line| line.starts_with("--bundle-stdin --devtools -- "))
        );
        assert!(!app.root.join(".apps").exists());
    }

    #[test]
    fn watch_waits_for_a_fix_when_the_first_build_fails() {
        let mut app = App::new();
        let entry = app.root.join("index.js");
        fs::write(&entry, "invalid javascript !!!").unwrap();
        app.child = Some(
            app.command()
                .arg("--watch")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        std::thread::sleep(Duration::from_millis(600));
        assert!(app.child.as_mut().unwrap().try_wait().unwrap().is_none());
        assert_eq!(app.launches(), "");
        fs::write(&entry, "console.log(1);").unwrap();
        app.wait(1);
    }

    #[test]
    fn long_watch_flag_restarts_after_successful_exit() {
        let mut app = App::new();
        let entry = app.root.join("custom.js");
        fs::write(&entry, "console.log(1);").unwrap();
        app.child = Some(
            app.command()
                .args(["custom.js", "--watch"])
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
        app.wait(1);
        fs::write(&entry, "console.log(2);").unwrap();
        app.wait(2);
        let line = format!("--bundle-stdin -- {}\n", entry.display());
        assert_eq!(app.launches(), line.repeat(2));
    }

    #[test]
    fn watches_public_assets_without_copying_them() {
        let mut app = App::new();
        fs::write(app.root.join("index.js"), "console.log(1);").unwrap();
        fs::create_dir(app.root.join("public")).unwrap();
        let asset = app.root.join("public/asset.txt");
        fs::write(&asset, "before").unwrap();
        app.child = Some(
            app.command()
                .arg("--watch")
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
        app.wait(1);
        fs::write(&asset, "after").unwrap();
        app.wait(2);
        assert!(!app.root.join(".apps").exists());
        assert_eq!(fs::read_to_string(asset).unwrap(), "after");
    }

    #[test]
    fn interrupt_stops_a_runtime_that_does_not_read_the_bundle() {
        let mut app = App::new();
        fs::write(
            app.directory.path().join("runtime"),
            "#!/bin/sh\nprintf '%s\\n' \"$$\" >> \"$APPS_RUN_LOG\"\nexec sleep 30\n",
        )
        .unwrap();
        fs::write(
            app.root.join("index.js"),
            format!("console.log({:?});", "large bundle".repeat(16384)),
        )
        .unwrap();
        app.child = Some(app.command().stdout(Stdio::null()).spawn().unwrap());
        app.wait(1);
        assert_eq!(app.stop().unwrap().code(), Some(130));
    }

    #[test]
    fn only_watch_restarts_running_apps_and_interrupt_stops_children() {
        for watch in [false, true] {
            let mut app = App::new();
            fs::write(
                app.directory.path().join("runtime"),
                "#!/bin/sh\ncat > \"$APPS_RUN_BUNDLE\"\nprintf '%s\\n' \"$$\" >> \"$APPS_RUN_LOG\"\nexec sleep 30\n",
            )
            .unwrap();
            let entry = app.root.join("index.js");
            fs::write(&entry, "console.log(1);").unwrap();
            app.child = Some(
                app.command()
                    .args(watch.then_some("--watch"))
                    .stdout(Stdio::null())
                    .spawn()
                    .unwrap(),
            );
            app.wait(1);
            fs::write(&entry, "console.log(2);").unwrap();
            if watch {
                app.wait(2);
            } else {
                std::thread::sleep(Duration::from_millis(600));
                assert_eq!(app.launches().lines().count(), 1);
            }
            assert_eq!(app.stop().unwrap().code(), Some(130));
            for pid in app.launches().lines() {
                assert!(
                    !Command::new("kill")
                        .args(["-0", pid])
                        .output()
                        .unwrap()
                        .status
                        .success(),
                    "App process {pid} is still running"
                );
            }
        }
    }
}
