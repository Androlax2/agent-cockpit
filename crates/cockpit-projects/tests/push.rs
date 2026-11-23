//! Pushing a checkout's branch only to a host its project may push to (#153, #857 decision 3),
//! as one resolved commit, forced only under the lease of decision 2, with the push token
//! offered to the push host alone (#155): through `cockpit_projects::push` on real Git, the
//! push going to a local bare repository that stands in for `https://github.com/owner/app.git`.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_forge_api::{ForgeHost, Token};
use cockpit_projects::{
    AgentBranches, BranchPattern, Error, Expected, Force, Git, PushCredential, PushRules, Pushed,
    Pusher, create_parallel_clone, push,
};
use cockpit_turso::Database;
use common::{Scratch, git, register};

const ORIGIN: &str = "https://github.com/owner/app.git";
const TOKEN: &str = "push-token-fixture";
/// The trailer the fixture's commits carry, as an agent's do (#694).
const AGENT_TRAILER: &str = "Agent: claude via Torlek s1";

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
            &[
                "init",
                "--quiet",
                "--bare",
                "--initial-branch=main",
                remote.to_str().unwrap(),
            ],
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
            pusher: Pusher::Owner { expected: None },
        };
        push(&self.database, &self.git, self.checkout, rules).await
    }

    /// The owner's push of `commit` on `branch` alone.
    async fn push_pinned(&self, branch: &str, commit: &str) -> Result<Pushed, Error> {
        let credential = PushCredential::new("x-access-token", Token::new(TOKEN).unwrap());
        let rules = PushRules {
            hosts: github(),
            credential: Ok(credential.unwrap()),
            force: Force::Never,
            pusher: Pusher::Owner {
                expected: Some(Expected {
                    branch: branch.to_owned(),
                    commit: commit.to_owned(),
                }),
            },
        };
        push(&self.database, &self.git, self.checkout, rules).await
    }

    /// A push as the agent of a checkout launched on `feature`, with no push-branch pattern.
    async fn push_as_agent(&self, force: Force) -> Result<Pushed, Error> {
        let branches = AgentBranches {
            launch: Some("feature".to_owned()),
            patterns: Vec::new(),
        };
        self.push_as_agent_of(self.checkout, branches, force).await
    }

    async fn push_as_agent_of(
        &self,
        checkout: cockpit_projects::CheckoutId,
        branches: AgentBranches,
        force: Force,
    ) -> Result<Pushed, Error> {
        let credential = PushCredential::new("x-access-token", Token::new(TOKEN).unwrap());
        let rules = PushRules {
            hosts: github(),
            credential: Ok(credential.unwrap()),
            force,
            pusher: Pusher::Agent { branches },
        };
        push(&self.database, &self.git, checkout, rules).await
    }

    /// Commits `content` to `path` in the checkout, with an agent's trailer.
    fn commit_file(&self, path: &str, content: &str) -> String {
        self.write_and_add(path, content);
        git(
            &self.root,
            &["commit", "--quiet", "-m", path, "--trailer", AGENT_TRAILER],
        );
        self.head()
    }

    /// Commits `content` to `path` in the checkout with no trailer, as the owner would.
    fn commit_file_unattributed(&self, path: &str, content: &str) -> String {
        self.write_and_add(path, content);
        git(&self.root, &["commit", "--quiet", "-m", path]);
        self.head()
    }

    fn write_and_add(&self, path: &str, content: &str) {
        let file = self.root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, content).unwrap();
        git(&self.root, &["add", path]);
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

    /// A commit someone else pushed straight to the bare repository's `branch`, writing
    /// `content` to `path`, on top of the branch or, for a new one, of `main`.
    fn someone_else_commits(&self, branch: &str, path: &str, content: &str) -> String {
        let other = self
            .scratch
            .path()
            .join(format!("other-{}", branch.replace('/', "-")));
        if !other.exists() {
            git(
                &self.scratch.path(),
                &[
                    "clone",
                    "--quiet",
                    self.remote.to_str().unwrap(),
                    other.to_str().unwrap(),
                ],
            );
        }
        git(&other, &["fetch", "--quiet", "origin"]);
        let start = if self.remote_tip(branch).is_some() {
            format!("origin/{branch}")
        } else {
            "origin/main".to_owned()
        };
        git(
            &other,
            &["switch", "--quiet", "--force-create", branch, &start],
        );
        let file = other.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, content).unwrap();
        git(&other, &["add", path]);
        git(&other, &["commit", "--quiet", "-m", path]);
        git(
            &other,
            &[
                "push",
                "--quiet",
                "origin",
                &format!("HEAD:refs/heads/{branch}"),
            ],
        );
        git(&other, &["rev-parse", "HEAD"]).trim().to_owned()
    }

    /// The remote calls Git made: `ls-remote`, `fetch` and `push`, each as one line starting
    /// with the folder Git ran in.
    fn remote_calls(&self) -> Vec<String> {
        self.recorded("remote-calls")
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
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
             pushing=; remote=; helper=\n\
             for argument; do\n  case \"$argument\" in\n    push) pushing=1 ;;\n    \
             ls-remote|fetch) remote=1 ;;\n    \
             credential.https://*) helper=\"$argument\" ;;\n  esac\ndone\n\
             if [ -n \"$pushing\" ]; then\n  printf '%s\\n' \"$@\" > {pushed}\n  \
             {{ {fill_push}; echo ---; {fill_other}; }} > {offered}\n  \
             if [ -x {before} ]; then {before}; fi\nfi\n\
             if [ -n \"$pushing$remote\" ]; then\n  printf '%s %s\\n' \"$PWD\" \"$*\" >> {remote_calls}\n  \
             export GIT_CONFIG_GLOBAL={rewrite} GIT_ALLOW_PROTOCOL=file\nfi\n\
             exec /usr/bin/git \"$@\"\n",
            home = quote(&scratch.path().join("git-home")),
            pushed = quote(&scratch.path().join("pushed")),
            offered = quote(&scratch.path().join("offered")),
            before = quote(&scratch.path().join("before-push")),
            remote_calls = quote(&scratch.path().join("remote-calls")),
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
            "push\n--porcelain\n--end-of-options\n{ORIGIN}\n{head}:refs/heads/main\n"
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

const WORKFLOW: &str = ".github/workflows/ci.yml";
const WORKFLOWS: &str = ".github/workflows";

fn workflow_change(result: Result<Pushed, Error>) -> (String, String, Vec<String>) {
    match result {
        Err(Error::WorkflowChange {
            branch,
            commit,
            paths,
            ..
        }) => (branch, commit, paths),
        other => panic!("expected a workflow change, got {other:?}"),
    }
}

/// A fixture whose `main` is on the remote, and a checkout switched to a new `feature`.
async fn on_a_new_feature_branch() -> Fixture {
    let fixture = Fixture::new(ORIGIN).await;
    fixture.push(github(), Force::Never).await.unwrap();
    git(&fixture.root, &["switch", "--quiet", "--create", "feature"]);
    fixture
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_new_branch_adding_a_workflow_file_waits_for_the_owner() {
    let fixture = on_a_new_feature_branch().await;
    let commit = fixture.commit_file(WORKFLOW, "on: push\n");
    fixture.forget_recorded_push();

    let (branch, refused_commit, paths) =
        workflow_change(fixture.push_as_agent(Force::Never).await);

    assert_eq!(
        (branch.as_str(), refused_commit.as_str()),
        ("feature", commit.as_str())
    );
    assert_eq!(paths, [WORKFLOWS]);
    assert_eq!(fixture.recorded("pushed"), None, "nothing is pushed");
    assert_eq!(fixture.remote_tip("feature"), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_workflow_change_names_the_owners_push_of_that_commit() {
    let fixture = on_a_new_feature_branch().await;
    let commit = fixture.commit_file(WORKFLOW, "on: push\n");

    let reason = fixture
        .push_as_agent(Force::Never)
        .await
        .unwrap_err()
        .to_string();

    let owners_push = format!(
        "`torlek checkout push {} --branch feature --commit {commit}`",
        fixture.checkout.0
    );
    assert!(reason.contains(&owners_push), "{reason}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owners_push_of_the_commit_held_for_an_agent_sends_it() {
    let fixture = on_a_new_feature_branch().await;
    let commit = fixture.commit_file(".gitlab-ci.yml", "test: {}\n");

    let pushed = fixture.push_pinned("feature", &commit).await.unwrap();

    assert_eq!(pushed.commit, commit);
    assert_eq!(fixture.remote_tip("feature"), Some(commit));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owners_push_of_a_commit_the_branch_moved_past_is_refused() {
    let fixture = on_a_new_feature_branch().await;
    let read = fixture.commit_file(WORKFLOW, "on: push\n");
    fixture.commit_file("README", "later\n");

    let reason = refusal(fixture.push_pinned("feature", &read).await);

    assert!(reason.contains("it moved since it was read"), "{reason}");
    assert_eq!(fixture.remote_tip("feature"), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owners_pinned_push_of_the_commit_on_another_branch_is_refused() {
    let fixture = on_a_new_feature_branch().await;
    let commit = fixture.commit_file(WORKFLOW, "on: push\n");
    let before = fixture.remote_tip("main");
    git(
        &fixture.root,
        &["checkout", "--quiet", "-B", "main", &commit],
    );

    let reason = refusal(fixture.push_pinned("feature", &commit).await);

    assert!(reason.contains("on main, not at"), "{reason}");
    assert_eq!(fixture.remote_tip("main"), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owners_pinned_push_of_an_abbreviated_commit_is_refused_as_not_a_full_id() {
    let fixture = on_a_new_feature_branch().await;
    let commit = fixture.commit_file("README", "work\n");

    let reason = refusal(fixture.push_pinned("feature", &commit[..7]).await);

    assert!(reason.contains("not a full commit id"), "{reason}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_clone_pushes_though_the_main_checkout_pushes_elsewhere() {
    let fixture = Fixture::new(ORIGIN).await;
    fixture.push(github(), Force::Never).await.unwrap();
    git(
        &fixture.root,
        &[
            "remote",
            "set-url",
            "--push",
            "origin",
            "git@github.com:owner/app.git",
        ],
    );
    let clone = create_parallel_clone(&fixture.database, &fixture.git, "app", "feature")
        .await
        .unwrap();
    std::fs::write(clone.path.join("README"), "clone work\n").unwrap();
    git(
        &clone.path,
        &[
            "commit",
            "--quiet",
            "-am",
            "work",
            "--trailer",
            AGENT_TRAILER,
        ],
    );
    let branches = AgentBranches {
        launch: Some("feature".to_owned()),
        patterns: Vec::new(),
    };

    let pushed = fixture
        .push_as_agent_of(clone.id, branches, Force::Never)
        .await
        .unwrap();

    assert_eq!(fixture.remote_tip("feature"), Some(pushed.commit));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_push_of_the_remotes_default_branch_is_refused() {
    let fixture = Fixture::new(ORIGIN).await;
    fixture.push(github(), Force::Never).await.unwrap();
    let before = fixture.remote_tip("main");
    fixture.commit_file("README", "agent work\n");
    let branches = AgentBranches {
        launch: Some("main".to_owned()),
        patterns: vec![BranchPattern::new("*").unwrap()],
    };

    let reason = refusal(
        fixture
            .push_as_agent_of(fixture.checkout, branches, Force::Never)
            .await,
    );

    assert!(reason.contains("the remote's default branch"), "{reason}");
    assert_eq!(fixture.remote_tip("main"), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_push_of_a_branch_neither_launched_nor_named_is_refused_before_the_remote() {
    let fixture = on_a_new_feature_branch().await;
    git(&fixture.root, &["switch", "--quiet", "--create", "other"]);
    fixture.commit_file("README", "work\n");
    let calls_before = fixture.remote_calls().len();

    let reason = refusal(fixture.push_as_agent(Force::Never).await);

    assert!(
        reason.contains("`torlek project push-branch add app PATTERN`"),
        "{reason}"
    );
    assert_eq!(fixture.remote_calls().len(), calls_before);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_push_of_a_branch_a_pattern_names_is_sent() {
    let fixture = on_a_new_feature_branch().await;
    git(
        &fixture.root,
        &["switch", "--quiet", "--create", "agent/fix/one"],
    );
    let commit = fixture.commit_file("README", "work\n");
    let branches = AgentBranches {
        launch: None,
        patterns: vec![BranchPattern::new("agent/**").unwrap()],
    };

    fixture
        .push_as_agent_of(fixture.checkout, branches, Force::Never)
        .await
        .unwrap();

    assert_eq!(fixture.remote_tip("agent/fix/one"), Some(commit));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_clone_whose_origin_names_another_repository_is_refused() {
    let fixture = Fixture::new(ORIGIN).await;
    fixture.push(github(), Force::Never).await.unwrap();
    let clone = create_parallel_clone(&fixture.database, &fixture.git, "app", "feature")
        .await
        .unwrap();
    git(
        &clone.path,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/someone/elsewhere.git",
        ],
    );
    let branches = AgentBranches {
        launch: Some("feature".to_owned()),
        patterns: Vec::new(),
    };

    let reason = refusal(
        fixture
            .push_as_agent_of(clone.id, branches, Force::Never)
            .await,
    );

    assert!(reason.contains("not the project's repository"), "{reason}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_clone_pushes_its_launch_branch_to_the_projects_repository() {
    let fixture = Fixture::new(ORIGIN).await;
    fixture.push(github(), Force::Never).await.unwrap();
    let clone = create_parallel_clone(&fixture.database, &fixture.git, "app", "feature")
        .await
        .unwrap();
    std::fs::write(clone.path.join("README"), "clone work\n").unwrap();
    git(
        &clone.path,
        &[
            "commit",
            "--quiet",
            "-am",
            "work",
            "--trailer",
            AGENT_TRAILER,
        ],
    );
    let branches = AgentBranches {
        launch: Some("feature".to_owned()),
        patterns: Vec::new(),
    };

    let pushed = fixture
        .push_as_agent_of(clone.id, branches, Force::Never)
        .await
        .unwrap();

    assert_eq!(fixture.remote_tip("feature"), Some(pushed.commit));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_remote_tips_are_read_outside_the_checkout_from_the_url_checked() {
    let fixture = on_a_new_feature_branch().await;
    fixture.commit_file("README", "work\n");

    fixture.push_as_agent(Force::Never).await.unwrap();

    let listed: Vec<String> = fixture
        .remote_calls()
        .into_iter()
        .filter(|call| call.contains(" ls-remote "))
        .collect();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert!(listed[0].starts_with("/ "), "{listed:?}");
    assert!(
        listed[0].contains(&format!("--end-of-options {ORIGIN} HEAD")),
        "{listed:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workflow_already_on_the_default_branch_is_not_held_on_a_new_branch() {
    let fixture = Fixture::new(ORIGIN).await;
    fixture.commit_file(WORKFLOW, "on: push\n");
    fixture.push(github(), Force::Never).await.unwrap();
    git(&fixture.root, &["switch", "--quiet", "--create", "feature"]);
    let commit = fixture.commit_file("README", "work\n");

    fixture.push_as_agent(Force::Never).await.unwrap();

    assert_eq!(fixture.remote_tip("feature"), Some(commit));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workflow_already_on_the_remote_branch_is_not_held_again() {
    let fixture = on_a_new_feature_branch().await;
    fixture.someone_else_commits("feature", WORKFLOW, "on: push\n");
    git(
        &fixture.root,
        &[
            "fetch",
            "--quiet",
            fixture.remote.to_str().unwrap(),
            "feature",
        ],
    );
    git(&fixture.root, &["reset", "--quiet", "--hard", "FETCH_HEAD"]);
    let commit = fixture.commit_file("README", "on top\n");

    fixture.push_as_agent(Force::Never).await.unwrap();

    assert_eq!(fixture.remote_tip("feature"), Some(commit));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rebase_bringing_in_the_workflow_of_the_default_branch_is_not_held() {
    let fixture = on_a_new_feature_branch().await;
    fixture.commit_file("README", "feature work\n");
    fixture.push_as_agent(Force::Never).await.unwrap();
    let main = fixture.someone_else_commits("main", WORKFLOW, "on: push\n");
    git(
        &fixture.root,
        &["fetch", "--quiet", fixture.remote.to_str().unwrap(), "main"],
    );
    git(&fixture.root, &["rebase", "--quiet", &main]);
    let rebased = fixture.head();

    let pushed = fixture.push_as_agent(lease_on("feature")).await.unwrap();

    assert!(pushed.forced);
    assert_eq!(fixture.remote_tip("feature"), Some(rebased));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_branch_predating_a_workflow_change_on_the_default_branch_is_held_until_rebased() {
    let fixture = on_a_new_feature_branch().await;
    let main = fixture.someone_else_commits("main", WORKFLOW, "on: push\n");
    fixture.commit_file("README", "work\n");

    let (_, _, paths) = workflow_change(fixture.push_as_agent(Force::Never).await);
    git(&fixture.root, &["rebase", "--quiet", &main]);
    let rebased = fixture.head();
    fixture.push_as_agent(Force::Never).await.unwrap();

    assert_eq!(paths, [WORKFLOWS]);
    assert_eq!(fixture.remote_tip("feature"), Some(rebased));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_default_branch_tip_the_checkout_lacks_is_fetched_by_id() {
    let fixture = on_a_new_feature_branch().await;
    let main = fixture.someone_else_commits("main", "README", "theirs\n");
    fixture.commit_file("NOTES", "work\n");

    fixture.push_as_agent(Force::Never).await.unwrap();

    let fetches: Vec<String> = fixture
        .remote_calls()
        .into_iter()
        .filter(|call| call.contains(" fetch "))
        .collect();
    assert_eq!(fetches.len(), 1, "{fetches:?}");
    assert!(
        fetches[0].ends_with(&format!("{ORIGIN} {main}")),
        "{fetches:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_push_to_an_empty_remote_is_refused() {
    let fixture = Fixture::new(ORIGIN).await;
    git(&fixture.root, &["switch", "--quiet", "--create", "feature"]);
    fixture.commit_file("README", "work\n");

    let reason = refusal(fixture.push_as_agent(Force::Never).await);

    assert!(
        reason.contains("the owner pushes the first branch"),
        "{reason}"
    );
    assert_eq!(fixture.remote_tip("feature"), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_a_workflow_is_held_like_adding_one() {
    let fixture = Fixture::new(ORIGIN).await;
    fixture.commit_file(WORKFLOW, "on: push\n");
    fixture.push(github(), Force::Never).await.unwrap();
    git(&fixture.root, &["switch", "--quiet", "--create", "feature"]);
    git(&fixture.root, &["rm", "--quiet", WORKFLOW]);
    git(
        &fixture.root,
        &[
            "commit",
            "--quiet",
            "-m",
            "drop ci",
            "--trailer",
            AGENT_TRAILER,
        ],
    );

    let (_, _, paths) = workflow_change(fixture.push_as_agent(Force::Never).await);

    assert_eq!(paths, [WORKFLOWS]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replace_ref_hiding_a_workflow_change_refuses_the_push() {
    let fixture = on_a_new_feature_branch().await;
    let main = git(&fixture.root, &["rev-parse", "main"]).trim().to_owned();
    let commit = fixture.commit_file(WORKFLOW, "on: push\n");
    git(&fixture.root, &["replace", &commit, &main]);

    let reason = refusal(fixture.push_as_agent(Force::Never).await);

    assert!(reason.contains("other content"), "{reason}");
    assert_eq!(fixture.remote_tip("feature"), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_graft_in_the_checkout_does_not_hide_a_workflow_change() {
    let fixture = on_a_new_feature_branch().await;
    let commit = fixture.commit_file(WORKFLOW, "on: push\n");
    let main = git(&fixture.root, &["rev-parse", "main"]).trim().to_owned();
    std::fs::write(
        fixture.root.join(".git/info/grafts"),
        format!("{main} {commit}\n"),
    )
    .unwrap();

    let (_, refused, paths) = workflow_change(fixture.push_as_agent(Force::Never).await);

    assert_eq!((refused, paths), (commit, vec![WORKFLOWS.to_owned()]));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owners_push_is_not_scanned_and_asks_the_remote_nothing_else() {
    let fixture = on_a_new_feature_branch().await;
    let commit = fixture.commit_file(WORKFLOW, "on: push\n");

    fixture.push(github(), Force::Never).await.unwrap();

    assert_eq!(fixture.remote_tip("feature"), Some(commit));
    let calls = fixture.remote_calls();
    assert_eq!(calls.len(), 2, "one push each: {calls:?}");
    assert!(
        calls.iter().all(|call| call.contains(" push ")),
        "{calls:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_commit_without_the_trailer_is_refused_naming_it_and_nothing_is_pushed() {
    let fixture = on_a_new_feature_branch().await;
    fixture.commit_file("README", "attributed\n");
    let unattributed = fixture.commit_file_unattributed("NOTES", "not attributed\n");
    fixture.forget_recorded_push();

    let reason = refusal(fixture.push_as_agent(Force::Never).await);

    assert!(
        reason.contains(&format!("commit {unattributed} carries no `Agent:")),
        "{reason}"
    );
    assert!(reason.contains("$TORLEK_COMMIT_TRAILER"), "{reason}");
    assert_eq!(fixture.recorded("pushed"), None, "nothing is pushed");
    assert_eq!(fixture.remote_tip("feature"), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_trailer_naming_no_session_does_not_attribute_a_commit() {
    let fixture = on_a_new_feature_branch().await;
    fixture.write_and_add("README", "work\n");
    git(
        &fixture.root,
        &[
            "commit",
            "--quiet",
            "-m",
            "work",
            "--trailer",
            "Agent: claude",
        ],
    );

    let reason = refusal(fixture.push_as_agent(Force::Never).await);

    assert!(reason.contains(&fixture.head()), "{reason}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_signed_commit_is_refused_naming_it() {
    let fixture = on_a_new_feature_branch().await;
    let parent = fixture.head();
    let tree = git(&fixture.root, &["rev-parse", "HEAD^{tree}"])
        .trim()
        .to_owned();
    let raw = format!(
        "tree {tree}\nparent {parent}\nauthor A <a@example.com> 1 +0000\n\
         committer A <a@example.com> 1 +0000\ngpgsig -----BEGIN SSH SIGNATURE-----\n \
         U1NIU0lH\n -----END SSH SIGNATURE-----\n\nsigned\n\n{AGENT_TRAILER}\n"
    );
    let raw_file = fixture.scratch.path().join("signed-commit");
    std::fs::write(&raw_file, raw).unwrap();
    let signed = git(
        &fixture.root,
        &[
            "hash-object",
            "-t",
            "commit",
            "-w",
            raw_file.to_str().unwrap(),
        ],
    )
    .trim()
    .to_owned();
    git(
        &fixture.root,
        &["update-ref", "refs/heads/feature", &signed],
    );

    let reason = refusal(fixture.push_as_agent(Force::Never).await);

    assert!(
        reason.contains(&format!("commit {signed} is signed")),
        "{reason}"
    );
    assert!(
        !reason.contains("no `Agent:"),
        "the commit is attributed: {reason}"
    );
    assert_eq!(fixture.remote_tip("feature"), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_refusal_rewrites_from_before_the_oldest_refused_commit() {
    let fixture = on_a_new_feature_branch().await;
    let oldest = fixture.commit_file_unattributed("README", "first\n");
    fixture.commit_file("NOTES", "attributed\n");
    fixture.commit_file_unattributed("TODO", "last\n");

    let reason = refusal(fixture.push_as_agent(Force::Never).await);

    assert!(
        reason.contains(&format!("--trailer \"$TORLEK_COMMIT_TRAILER\"' {oldest}^`")),
        "{reason}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn commits_another_remote_branch_holds_need_no_trailer() {
    let fixture = on_a_new_feature_branch().await;
    let stacked_on = fixture.someone_else_commits("base", "README", "the owner's\n");
    git(
        &fixture.root,
        &["fetch", "--quiet", fixture.remote.to_str().unwrap(), "base"],
    );
    git(&fixture.root, &["reset", "--quiet", "--hard", &stacked_on]);
    let commit = fixture.commit_file("NOTES", "the agent's\n");

    fixture.push_as_agent(Force::Never).await.unwrap();

    assert_eq!(fixture.remote_tip("feature"), Some(commit));
}

#[tokio::test(flavor = "multi_thread")]
async fn commits_the_remote_branch_already_holds_need_no_trailer() {
    let fixture = on_a_new_feature_branch().await;
    fixture.commit_file_unattributed("README", "the owner's\n");
    fixture.push(github(), Force::Never).await.unwrap();
    let commit = fixture.commit_file("NOTES", "the agent's\n");

    fixture.push_as_agent(Force::Never).await.unwrap();

    assert_eq!(fixture.remote_tip("feature"), Some(commit));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owners_push_of_an_unattributed_commit_is_sent() {
    let fixture = on_a_new_feature_branch().await;
    let commit = fixture.commit_file_unattributed("README", "the owner's\n");

    fixture.push(github(), Force::Never).await.unwrap();

    assert_eq!(fixture.remote_tip("feature"), Some(commit));
}

#[test]
fn a_username_that_is_not_one_plain_word_is_refused() {
    for (case, username) in ["", "two words", "quote'd", "semi;colon"]
        .into_iter()
        .enumerate()
    {
        let token = Token::new(TOKEN).unwrap();
        assert!(
            matches!(
                PushCredential::new(username, token),
                Err(Error::Refused { .. })
            ),
            "case {case}"
        );
    }
}
