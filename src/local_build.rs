//! The `docker run` command that reproduces a GitOps kaniko build on a
//! workstation (MCP `generate_local_build`).
//!
//! Most of what goes into the command comes from stored GitOps config (the
//! Dockerfile path, the build context, the OCI repository), which anyone who
//! can configure GitOps for a deployment controls, and the rest from build
//! settings and the caller. The command is meant to be pasted into a shell
//! or run by an agent, so every argument is quoted for a POSIX shell
//! ([`quote`]): no value can add a command, a substitution or an expansion.
//! The arguments are also returned as a list, for running without a shell.

use std::borrow::Cow;

use crate::handlers::settings::BuildSettings;

/// What a local build needs.
pub struct LocalBuild<'a> {
    /// Target platform, `docker run --platform` and kaniko's platform flag.
    pub platform: &'a str,
    /// The caller's path to the repository (or build context) root.
    pub local_context: &'a str,
    /// The stored build context, relative to the repository root.
    pub docker_context: &'a str,
    /// The stored Dockerfile path; only its file name is used.
    pub dockerfile_path: &'a str,
    /// The stored OCI repository, pushed to as `:local-test` unless `no_push`.
    pub oci_repository: &'a str,
    pub no_push: bool,
    pub settings: &'a BuildSettings,
}

/// The command: its arguments (`docker` first) and the same command quoted
/// for a POSIX shell.
pub struct Command {
    pub argv: Vec<String>,
    pub shell: String,
}

impl LocalBuild<'_> {
    pub fn command(&self) -> Command {
        let bs = self.settings;
        let context = if self.docker_context != "." {
            format!("{}/{}", self.local_context, self.docker_context)
        } else {
            self.local_context.to_string()
        };
        let dockerfile = self
            .dockerfile_path
            .rsplit('/')
            .next()
            .unwrap_or(self.dockerfile_path);

        // `docker run` and its options, one group per line.
        let docker: Vec<Vec<String>> = vec![
            vec!["docker".into(), "run".into(), "--rm".into()],
            vec!["--platform".into(), self.platform.into()],
            vec!["-v".into(), format!("{context}:/workspace")],
            vec![bs.kaniko_image.clone()],
        ];
        let mut kaniko = vec![
            format!("--dockerfile=/workspace/{dockerfile}"),
            "--context=dir:///workspace".to_string(),
            format!("{}={}", bs.platform_flag, self.platform),
            format!("--snapshot-mode={}", bs.snapshot_mode),
        ];
        if bs.cache_enabled {
            kaniko.push("--cache=true".to_string());
        }
        if self.no_push {
            kaniko.push("--no-push".to_string());
        } else {
            kaniko.push(format!("--destination={}:local-test", self.oci_repository));
        }
        kaniko.extend(bs.extra_kaniko_args.iter().cloned());

        let mut lines: Vec<String> = docker
            .iter()
            .map(|words| {
                words
                    .iter()
                    .map(|w| quote_home_path(w))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        lines.extend(kaniko.iter().map(|arg| format!("  {}", quote(arg))));

        Command {
            argv: docker.into_iter().flatten().chain(kaniko).collect(),
            shell: lines.join(" \\\n"),
        }
    }
}

/// `word` quoted for a POSIX shell (sh, bash, zsh): unchanged when it is made
/// only of characters no shell treats specially, otherwise inside single
/// quotes, where nothing is special, with each `'` written as `'\''`.
pub fn quote(word: &str) -> Cow<'_, str> {
    let plain = !word.is_empty()
        // zsh expands a leading `=` (`=cmd` is the path of cmd).
        && !word.starts_with('=')
        && word.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '-' | '_' | '.' | '/' | ':' | '=' | '@' | '%' | '+' | ',')
        });
    if plain {
        Cow::Borrowed(word)
    } else {
        Cow::Owned(format!("'{}'", word.replace('\'', r"'\''")))
    }
}

/// [`quote`], except that a leading `~/` stays outside the quotes, so a
/// home-relative path still expands. Only the tilde-prefix is unquoted, and
/// tilde expansion runs no code.
fn quote_home_path(word: &str) -> Cow<'_, str> {
    match word.strip_prefix("~/") {
        Some(rest) => Cow::Owned(format!("~/{}", quote(rest))),
        None => quote(word),
    }
}

#[cfg(test)]
#[path = "local_build_tests.rs"]
mod tests;
