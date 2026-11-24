//! What Qwen Code needs from the sandbox (#830) and where it keeps and takes its credentials
//! (#858), read from Qwen Code 0.28.4's source: its provider presets
//! (`packages/core/src/providers/presets/`), its OAuth configuration
//! (`packages/core/src/qwen/oauth-config.ts`) and its standalone installer
//! (`scripts/installation/install-qwen-standalone.sh`). Linux paths only.

use cockpit_provider_api::{CredentialHeader, Credentials, InvalidValue, Needs};

/// Qwen OAuth and the Qwen subscription: the model API, the token refresh and the sign-in page.
const QWEN_HOSTS: [&str; 3] = ["api.qwen.ai", "auth.qwen.ai", "chat.qwen.ai"];

/// Alibaba Cloud Model Studio (DashScope), each region of its `DASHSCOPE_API_KEY` preset.
const DASHSCOPE_HOSTS: [&str; 4] = [
    "dashscope.aliyuncs.com",
    "dashscope-intl.aliyuncs.com",
    "dashscope-us.aliyuncs.com",
    "cn-hongkong.dashscope.aliyuncs.com",
];

/// Alibaba Cloud's Coding Plan, `BAILIAN_CODING_PLAN_API_KEY`.
const CODING_PLAN_HOSTS: [&str; 2] = [
    "coding.dashscope.aliyuncs.com",
    "coding-intl.dashscope.aliyuncs.com",
];

/// Alibaba Cloud's Token Plan, `BAILIAN_TOKEN_PLAN_API_KEY`.
const TOKEN_PLAN_HOSTS: [&str; 2] = [
    "token-plan.cn-beijing.maas.aliyuncs.com",
    "token-plan.ap-southeast-1.maas.aliyuncs.com",
];

/// QwenCloud, `QWENCLOUD_API_KEY`.
const QWENCLOUD_HOSTS: [&str; 2] = ["maas.qianwenaiapi.com", "maas.qwencloudapi.com"];

/// QwenCloud's Token Plan, `QWENCLOUD_TOKEN_PLAN_API_KEY`.
const QWENCLOUD_TOKEN_PLAN_HOSTS: [&str; 2] = [
    "token-plan.maas.qianwenaiapi.com",
    "token-plan.maas.qwencloudapi.com",
];

/// The hosts Qwen Code reaches for its first-party sign-ins and keys. Left out: its usage
/// statistics host (telemetry, which fails closed), `models.dev` (an optional catalog refresh)
/// and `registry.npmjs.org` (the update check, which a launch turns off). Another provider of
/// the owner's `modelProviders`, or a local server, is the owner's entry or `[[services]]`.
pub fn hosts() -> Vec<&'static str> {
    [
        &QWEN_HOSTS[..],
        &DASHSCOPE_HOSTS,
        &CODING_PLAN_HOSTS,
        &TOKEN_PLAN_HOSTS,
        &QWENCLOUD_HOSTS,
        &QWENCLOUD_TOKEN_PLAN_HOSTS,
    ]
    .concat()
}

/// Read from the owner's home: the standalone installer's default install, which holds its
/// own Node. An npm install under the home is the owner's entry to add.
pub const READ: [&str; 1] = ["~/.local/lib/qwen-code"];

/// Written into the session's own folders: Qwen Code's settings, chats and temporary files,
/// and its cache.
pub const WRITE: [&str; 2] = ["~/.qwen", "~/.cache/qwen-code"];

/// Where Qwen OAuth keeps its sign-in.
pub const CREDENTIAL_FILES: [&str; 1] = ["~/.qwen/oauth_creds.json"];

/// Each provider key variable of Qwen Code's first-party presets, with the hosts that take it
/// as Bearer (their OpenAI-compatible APIs).
pub fn credential_bindings() -> [(&'static str, CredentialHeader, &'static [&'static str]); 5] {
    [
        (
            "DASHSCOPE_API_KEY",
            CredentialHeader::Bearer,
            &DASHSCOPE_HOSTS,
        ),
        (
            "BAILIAN_CODING_PLAN_API_KEY",
            CredentialHeader::Bearer,
            &CODING_PLAN_HOSTS,
        ),
        (
            "BAILIAN_TOKEN_PLAN_API_KEY",
            CredentialHeader::Bearer,
            &TOKEN_PLAN_HOSTS,
        ),
        (
            "QWENCLOUD_API_KEY",
            CredentialHeader::Bearer,
            &QWENCLOUD_HOSTS,
        ),
        (
            "QWENCLOUD_TOKEN_PLAN_API_KEY",
            CredentialHeader::Bearer,
            &QWENCLOUD_TOKEN_PLAN_HOSTS,
        ),
    ]
}

pub fn needs() -> Result<Needs, InvalidValue> {
    Needs::new(&hosts(), &READ, &WRITE)
}

pub fn credentials() -> Result<Credentials, InvalidValue> {
    Credentials::new(&CREDENTIAL_FILES, &credential_bindings(), &needs()?)
}
