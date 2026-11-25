use super::*;

/// Build settings as a fresh install has them (the serde defaults).
fn default_settings() -> BuildSettings {
    serde_json::from_value(serde_json::json!({})).unwrap()
}

fn build<'a>(settings: &'a BuildSettings) -> LocalBuild<'a> {
    LocalBuild {
        platform: "linux/amd64",
        local_context: ".",
        docker_context: "app",
        dockerfile_path: "app/Dockerfile",
        oci_repository: "registry.example:5000/team/web",
        no_push: true,
        settings,
    }
}

/// Values that run code, split words or expand when they reach a shell
/// unquoted. None of them may do anything but arrive as one argument.
const HOSTILE: &[&str] = &[
    "$(touch canary)",
    "`touch canary`",
    "a;touch canary",
    "a && touch canary",
    "a|touch canary",
    "a\ntouch canary",
    "it's",
    "'; touch canary; '",
    "$HOME",
    "${HOME}",
    "*",
    "~",
    "a b",
    "=ls",
    "!!",
    "#comment",
    "\\",
    "",
];

#[test]
fn plain_words_are_left_unquoted() {
    for word in [
        "docker",
        "--rm",
        "linux/amd64",
        "./app:/workspace",
        "--dockerfile=/workspace/Dockerfile",
        "gcr.io/kaniko-project/executor:v1.24.0",
        "--destination=registry.example:5000/team/web:local-test",
        "a,b+c@d%e",
    ] {
        assert_eq!(quote(word), word);
    }
}

#[test]
fn everything_else_is_single_quoted() {
    assert_eq!(quote(""), "''");
    assert_eq!(quote("a b"), "'a b'");
    assert_eq!(quote("$(id)"), "'$(id)'");
    assert_eq!(quote("it's"), r"'it'\''s'");
    assert_eq!(quote("=ls"), "'=ls'");
    assert_eq!(quote("~/x"), "'~/x'");
}

/// Run `script` with `sh -c` in `dir` and return its output.
#[cfg(unix)]
fn sh(script: &str, dir: &std::path::Path) -> String {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(script)
        .current_dir(dir)
        .env("HOME", "/home/test")
        .output()
        .expect("sh runs");
    assert!(out.status.success(), "{script}\n{out:?}");
    String::from_utf8(out.stdout).unwrap()
}

#[cfg(unix)]
#[test]
fn a_quoted_word_reaches_the_shell_as_itself() {
    let dir = tempfile::tempdir().unwrap();
    for value in HOSTILE {
        let printed = sh(&format!("printf '%s|' {}", quote(value)), dir.path());
        assert_eq!(printed, format!("{value}|"), "value {value:?}");
    }
    assert!(!dir.path().join("canary").exists(), "a value ran a command");
}

/// The arguments `docker` would get from the shell command, by running it
/// with `printf` in place of `docker`.
#[cfg(unix)]
fn shell_argv(command: &Command, dir: &std::path::Path) -> Vec<String> {
    let script = command
        .shell
        .strip_prefix("docker ")
        .expect("the command starts with docker");
    let printed = sh(&format!("printf '%s\\0' {script}"), dir);
    printed.split_terminator('\0').map(str::to_string).collect()
}

#[cfg(unix)]
#[test]
fn hostile_gitops_and_settings_values_stay_single_arguments() {
    let dir = tempfile::tempdir().unwrap();
    for value in HOSTILE {
        let mut settings = default_settings();
        settings.kaniko_image = format!("kaniko{value}");
        settings.snapshot_mode = value.to_string();
        settings.platform_flag = format!("--custom-platform{value}");
        settings.extra_kaniko_args = vec![value.to_string(), format!("--label={value}")];
        for no_push in [true, false] {
            let local = LocalBuild {
                platform: value,
                local_context: value,
                docker_context: value,
                dockerfile_path: &format!("docker/{value}"),
                oci_repository: value,
                no_push,
                settings: &settings,
            };
            let command = local.command();
            assert_eq!(command.argv[0], "docker");
            // What the shell should pass on: the arguments as they are, but
            // for the one thing left unquoted on purpose, a leading "~/" in
            // the volume (which "~" as the local context produces).
            let mut expected = command.argv[1..].to_vec();
            if let Some(rest) = expected[5].strip_prefix("~/") {
                expected[5] = format!("/home/test/{rest}");
            }
            assert_eq!(
                shell_argv(&command, dir.path()),
                expected,
                "value {value:?}:\n{}",
                command.shell
            );
        }
    }
    assert!(!dir.path().join("canary").exists(), "a value ran a command");
}

#[cfg(unix)]
#[test]
fn a_home_relative_local_context_still_expands() {
    let dir = tempfile::tempdir().unwrap();
    let settings = default_settings();
    let local = LocalBuild {
        local_context: "~/src/my app",
        ..build(&settings)
    };
    let command = local.command();
    assert!(
        command.shell.contains("-v ~/'src/my app/app:/workspace'"),
        "{}",
        command.shell
    );
    let argv = shell_argv(&command, dir.path());
    assert_eq!(argv[4], "-v");
    assert_eq!(argv[5], "/home/test/src/my app/app:/workspace");
}

#[test]
fn ordinary_values_render_as_before() {
    let mut settings = default_settings();
    settings.extra_kaniko_args = vec!["--verbosity=debug".into()];
    let command = build(&settings).command();
    let expected = format!(
        "docker run --rm \\\n\
         --platform linux/amd64 \\\n\
         -v ./app:/workspace \\\n\
         {image} \\\n  \
         --dockerfile=/workspace/Dockerfile \\\n  \
         --context=dir:///workspace \\\n  \
         {flag}=linux/amd64 \\\n  \
         --snapshot-mode={mode} \\\n  \
         --cache=true \\\n  \
         --no-push \\\n  \
         --verbosity=debug",
        image = settings.kaniko_image,
        flag = settings.platform_flag,
        mode = settings.snapshot_mode,
    );
    assert_eq!(command.shell, expected);
    assert_eq!(
        command.argv,
        [
            "docker",
            "run",
            "--rm",
            "--platform",
            "linux/amd64",
            "-v",
            "./app:/workspace",
            settings.kaniko_image.as_str(),
            "--dockerfile=/workspace/Dockerfile",
            "--context=dir:///workspace",
            format!("{}=linux/amd64", settings.platform_flag).as_str(),
            format!("--snapshot-mode={}", settings.snapshot_mode).as_str(),
            "--cache=true",
            "--no-push",
            "--verbosity=debug",
        ]
    );

    // Pushing names the stored repository; a "." context mounts the local one.
    let local = LocalBuild {
        docker_context: ".",
        no_push: false,
        ..build(&settings)
    };
    let command = local.command();
    assert!(
        command.shell.contains("-v .:/workspace \\\n"),
        "{}",
        command.shell
    );
    assert!(command
        .argv
        .contains(&"--destination=registry.example:5000/team/web:local-test".to_string()));
}
