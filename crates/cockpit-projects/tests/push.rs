//! Pushing a checkout's branch only to a host its project may push to (#153, #857 decision 3),
//! as one resolved commit, forced only under the lease of decision 2, with the push token
//! offered to the push host alone (#155): through `cockpit_projects::push` on real Git, the
//! push going to a local bare repository that stands in for `https://github.com/owner/app.git`.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_forge_api::{ForgeHost, Token};
use cockpit_projects::{Error, Force, Git, PushCredential, PushRules, Pushed, push};
use cockpit_turso::Database;
use common::{Scratch, git, register};

const ORIGIN: &str = "https://github.com/owner/app.git";
const TOKEN: &str = "push-token-fixture";

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

/// One project whose main checkout's origin is `origin`, and a Git that sends its pushes to a
/// bare repository instead of the forge: real Git for every call, with `origin`'s URL rewritten
/// to the bare repository for the push alone, so the checks before it still read `origin`.
/// Before each push it writes the push's arguments to `pushed` and what Git's credential lookup
/// answers, under the push's own settings, for the push host and for another host to `offered`.
/// An executable `before-push` in the scratch directory runs first.
struct Fixture {
    scratch: Scratch,
    root: PathBuf,
    remote: PathBuf,
    database: Database,
    checkout: cockpit_projects::CheckoutId,
    git: Git,
}

impl Fixture {
    async fn new(origin: &str) -> Fixture {
        let scratch = Scratch::new();
        let root = scratch.repository("app");
        git(&root, &["remote", "add", "origin", origin]);
        let remote = scratch.path().join("remote.git");
        git(
            &scratch.path(),
            &["init", "--quiet", "--bare", remote.to_str().unwrap()],
        );
        let database = scratch.open().await;
        let (_, checkout) = register(&database, &root).await;
        let git = forge_git(&scratch, &remote);
        Fixture {
            scratch,
            root,
            remote,
            database,
            checkout: checkout.id,
            git,
        }
    }

    async fn push(&self, hosts: Vec<ForgeHost>, force: Force) -> Result<Pushed, Error> {
        let credential = PushCredential::new("x-access-token", Token::new(TOKEN).unwrap());
        self.push_with(hosts, Ok(credential.unwrap()), force).await
    }

    async fn push_with(
        &self,
        hosts: Vec<ForgeHost>,
        credential: Result<PushCredential, String>,
        force: Force,
    ) -> Result<Pushed, Error> {
        let rules = PushRules {
            hosts,
            credential,
            force,
        };
        push(&self.database, &self.git, self.checkout, rules).await
    }

    fn recorded(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.scratch.path().join(name)).ok()
    }

    fn forget_recorded_push(&self) {
        let _ = std::fs::remove_file(self.scratch.path().join("pushed"));
    }

    /// The commit `branch` is at on the bare repository; `None` when it has no such branch.
    fn remote_tip(&self, branch: &str) -> Option<String> {
        let output = std::process::Command::new("git")
            .arg("--git-dir")
            .arg(&self.remote)
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .output()
            .unwrap();
        output
            .status
            .success()
            .then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned())
    }

    fn head(&self) -> String {
        git(&self.root, &["rev-parse", "HEAD"]).trim().to_owned()
    }

    /// Rewrites the checkout's last commit, so pushing it is no fast-forward.
    fn rewrite_last_commit(&self) {
        git(
            &self.root,
            &[
                "commit",
                "--quiet",
                "--amend",
                "--allow-empty",
                "-m",
                "rewritten",
            ],
        );
    }

    /// A commit someone else pushed straight to the bare repository's `main`.
    fn someone_else_pushes(&self) -> String {
        let other = self.scratch.path().join("other");
        git(
            &self.scratch.path(),
            &[
                "clone",
                "--quiet",
                self.remote.to_str().unwrap(),
                other.to_str().unwrap(),
            ],
        );
        git(
            &other,
            &["commit", "--quiet", "--allow-empty", "-m", "theirs"],
        );
        git(
            &other,
            &["push", "--quiet", "origin", "HEAD:refs/heads/main"],
        );
        git(&other, &["rev-parse", "HEAD"]).trim().to_owned()
    }
}

fn forge_git(scratch: &Scratch, remote: &Path) -> Git {
    let wrapper = scratch.path().join("git-forge");
    let rewrite = scratch.path().join("rewrite.gitconfig");
    std::fs::write(
        &rewrite,
        format!(
            "[url \"{}\"]\n\tinsteadOf = {ORIGIN}\n",
            remote.to_str().unwrap()
        ),
    )
    .unwrap();
    let fill = |host: &str| {
        format!(
            "printf 'protocol=https\\nhost={host}\\n\\n' | /usr/bin/git -c credential.helper= \
             -c \"$helper\" credential fill 2>&1"
        )
    };
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexport HOME={home} GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1\n\
             pushing=; helper=\n\
             for argument; do\n  case \"$argument\" in\n    push) pushing=1 ;;\n    \
             credential.https://*) helper=\"$argument\" ;;\n  esac\ndone\n\
             if [ -n \"$pushing\" ]; then\n  printf '%s\\n' \"$@\" > {pushed}\n  \
             {{ {fill_push}; echo ---; {fill_other}; }} > {offered}\n  \
             if [ -x {before} ]; then {before}; fi\n  \
             export GIT_CONFIG_GLOBAL={rewrite} GIT_ALLOW_PROTOCOL=file\nfi\n\
             exec /usr/bin/git \"$@\"\n",
            home = quote(&scratch.path().join("git-home")),
            pushed = quote(&scratch.path().join("pushed")),
            offered = quote(&scratch.path().join("offered")),
            before = quote(&scratch.path().join("before-push")),
            rewrite = quote(&rewrite),
            fill_push = fill("github.com"),
            fill_other = fill("forge.example.com"),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    Git::with_program(wrapper)
}

fn github() -> Vec<ForgeHost> {
    vec![ForgeHost::new("github.com").unwrap()]
}

fn lease_on(branch: &str) -> Force {
    Force::Lease {
        own_branch: branch.to_owned(),
    }
}

fn refusal(result: Result<Pushed, Error>) -> String {
    match result {
        Err(Error::Refused { reason }) => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_branch_is_pushed_as_the_commit_it_points_at_to_a_host_the_project_may_push_to() {
    let fixture = Fixture::new(ORIGIN).await;
    let head = fixture.head();

    let pushed = fixture.push(github(), Force::Never).await.unwrap();

    assert_eq!(
        pushed,
        Pushed {
            branch: "main".to_owned(),
            commit: head.clone(),
            remote_url: ORIGIN.to_owned(),
            forced: false,
        }
    );
    assert_eq!(fixture.remote_tip("main"), Some(head.clone()));
    let arguments = fixture.recorded("pushed").unwrap();
    assert!(
        arguments.ends_with(&format!(
            "push\n--porcelain\n--end-of-options\norigin\n{head}:refs/heads/main\n"
        )),
        "{arguments}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pushed_branch_tracks_the_remote_branch() {
    let fixture = Fixture::new(ORIGIN).await;

    fixture.push(github(), Force::Never).await.unwrap();

    let upstream = git(
        &fixture.root,
        &["config", "--get-regexp", r"^branch\.main\."],
    );
    assert_eq!(
        upstream,
        "branch.main.remote origin\nbranch.main.merge refs/heads/main\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_branch_moved_once_the_push_began_still_sends_the_commit_read_first() {
    let fixture = Fixture::new(ORIGIN).await;
    let read_first = fixture.head();
    let mover = fixture.scratch.path().join("before-push");
    std::fs::write(
        &mover,
        format!(
            "#!/bin/sh\nexec /usr/bin/git -C {} -c user.name=Agent -c user.email=agent@example.com \
             commit --quiet --allow-empty -m moved\n",
            quote(&fixture.root)
        ),
    )
    .unwrap();
    std::fs::set_permissions(&mover, std::fs::Permissions::from_mode(0o700)).unwrap();

    let pushed = fixture.push(github(), Force::Never).await.unwrap();

    assert_ne!(fixture.head(), read_first, "the branch moved");
    assert_eq!(pushed.commit, read_first);
    assert_eq!(fixture.remote_tip("main"), Some(read_first));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_push_token_is_offered_to_the_push_host_alone_and_never_in_an_argument() {
    let fixture = Fixture::new(ORIGIN).await;

    fixture.push(github(), Force::Never).await.unwrap();

    let offered = fixture.recorded("offered").unwrap();
    let (push_host, other_host) = offered.split_once("---\n").unwrap();
    assert!(
        push_host.contains(&format!("username=x-access-token\npassword={TOKEN}\n")),
        "{push_host}"
    );
    assert!(!other_host.contains(TOKEN), "{other_host}");
    assert!(!fixture.recorded("pushed").unwrap().contains(TOKEN));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rewritten_branch_is_refused_without_a_lease() {
    let fixture = Fixture::new(ORIGIN).await;
    let first = fixture.push(github(), Force::Never).await.unwrap().commit;
    fixture.rewrite_last_commit();

    let result = fixture.push(github(), Force::Never).await;

    assert!(
        matches!(&result, Err(Error::Git { stderr, .. }) if stderr.contains("non-fast-forward")),
        "{result:?}"
    );
    assert_eq!(fixture.remote_tip("main"), Some(first));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lease_force_on_the_own_branch_replaces_the_commit_the_daemon_last_pushed() {
    let fixture = Fixture::new(ORIGIN).await;
    let first = fixture.push(github(), Force::Never).await.unwrap().commit;
    fixture.rewrite_last_commit();

    let pushed = fixture.push(github(), lease_on("main")).await.unwrap();

    assert!(pushed.forced);
    assert_eq!(fixture.remote_tip("main"), Some(fixture.head()));
    let arguments = fixture.recorded("pushed").unwrap();
    assert!(
        arguments.contains(&format!("--force-with-lease=refs/heads/main:{first}\n")),
        "{arguments}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lease_force_is_refused_by_git_once_the_remote_moved_past_the_recorded_push() {
    let fixture = Fixture::new(ORIGIN).await;
    fixture.push(github(), Force::Never).await.unwrap();
    let theirs = fixture.someone_else_pushes();
    fixture.rewrite_last_commit();

    let result = fixture.push(github(), lease_on("main")).await;

    assert!(
        matches!(&result, Err(Error::Git { stderr, .. }) if stderr.contains("stale info")),
        "{result:?}"
    );
    assert_eq!(fixture.remote_tip("main"), Some(theirs));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lease_force_with_no_recorded_push_is_refused_never_taking_the_remote_tip() {
    let fixture = Fixture::new(ORIGIN).await;
    let theirs = fixture.someone_else_pushes();

    let reason = refusal(fixture.push(github(), lease_on("main")).await);

    assert!(reason.contains("no push of main"), "{reason}");
    assert_eq!(fixture.recorded("pushed"), None, "nothing is pushed");
    assert_eq!(fixture.remote_tip("main"), Some(theirs));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lease_force_on_another_branch_than_the_own_one_is_refused() {
    let fixture = Fixture::new(ORIGIN).await;
    fixture.push(github(), Force::Never).await.unwrap();
    fixture.forget_recorded_push();
    fixture.rewrite_last_commit();

    let reason = refusal(fixture.push(github(), lease_on("feature")).await);

    assert!(
        reason.contains("only to the session's own branch feature"),
        "{reason}"
    );
    assert_eq!(fixture.recorded("pushed"), None, "nothing is pushed");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_without_the_push_right_is_refused_naming_the_command_that_binds_it() {
    for push_hosts in [vec![], vec![ForgeHost::new("forge.example.com").unwrap()]] {
        let fixture = Fixture::new(ORIGIN).await;

        let reason = refusal(fixture.push(push_hosts.clone(), Force::Never).await);

        assert!(
            reason.contains("`torlek network grant app github.com --push`"),
            "{push_hosts:?}: {reason}"
        );
        assert_eq!(
            fixture.recorded("pushed"),
            None,
            "{push_hosts:?}: nothing is pushed"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ssh_origin_is_refused_naming_ssh_even_on_a_host_the_project_may_push_to() {
    for origin in [
        "git@github.com:owner/app.git",
        "ssh://git@github.com/owner/app.git",
    ] {
        let fixture = Fixture::new(origin).await;

        let reason = refusal(fixture.push(github(), Force::Never).await);

        assert!(reason.contains("SSH"), "{origin}: {reason}");
        assert_eq!(
            fixture.recorded("pushed"),
            None,
            "{origin}: nothing is pushed"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_origin_naming_a_credential_is_refused() {
    let fixture = Fixture::new("https://x-access-token:SECRET@github.com/owner/app.git").await;

    let reason = refusal(fixture.push(github(), Force::Never).await);

    assert!(reason.contains("names a user or a password"), "{reason}");
    assert!(
        !reason.contains("SECRET"),
        "the credential is not repeated: {reason}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_without_a_credential_is_refused_for_it_only_once_its_origin_passed() {
    let no_token = || Err("no push token here".to_owned());
    let ssh = Fixture::new("git@github.com:owner/app.git").await;
    let https = Fixture::new(ORIGIN).await;

    let ssh_reason = refusal(ssh.push_with(github(), no_token(), Force::Never).await);
    let https_reason = refusal(https.push_with(github(), no_token(), Force::Never).await);

    assert!(ssh_reason.contains("SSH"), "{ssh_reason}");
    assert_eq!(https_reason, "no push token here");
    assert_eq!(https.recorded("pushed"), None, "nothing is pushed");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lease_allowed_on_a_fast_forward_reports_no_force() {
    let fixture = Fixture::new(ORIGIN).await;
    fixture.push(github(), Force::Never).await.unwrap();
    git(
        &fixture.root,
        &["commit", "--quiet", "--allow-empty", "-m", "next"],
    );

    let pushed = fixture.push(github(), lease_on("main")).await.unwrap();

    assert!(!pushed.forced);
    assert_eq!(fixture.remote_tip("main"), Some(fixture.head()));
}

#[test]
fn a_username_that_is_not_one_plain_word_is_refused() {
    for username in ["", "two words", "quote'd", "semi;colon"] {
        let token = Token::new(TOKEN).unwrap();
        assert!(
            matches!(
                PushCredential::new(username, token),
                Err(Error::Refused { .. })
            ),
            "{username:?}"
        );
    }
}
