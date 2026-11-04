//! Which published release, if any, this installation may be offered (#617): semantic version
//! precedence, the alpha, beta and stable channels, and the GitHub release list read as data.
//! Nothing here touches the network or the disk; the caller hands in the list as text.

use std::cmp::Ordering;
use std::fmt;

use serde_json::Value;

/// Release notes are shown as a short excerpt, so the cache never holds more than this.
const NOTES_LIMIT: usize = 4000;

const CHECKSUMS_ASSET: &str = "SHA256SUMS";

/// A semantic version without build metadata, which no Torlek tag carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Version {
    core: [u64; 3],
    prerelease: Vec<Identifier>,
}

/// Numeric identifiers sort before textual ones, as semantic versioning orders them: the
/// variant order below is that rule.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Identifier {
    Numeric(u64),
    Text(String),
}

/// The release channels of [Releasing](../../../../docs/RELEASING.md), oldest first. A newer
/// channel is the next step for an older one; the other direction is never offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Channel {
    Alpha,
    Beta,
    Stable,
}

impl Version {
    pub(super) fn parse(text: &str) -> Result<Self, String> {
        let invalid = |reason: &str| format!("{text:?} is not a release version: {reason}");
        let (core_text, prerelease_text) = match text.split_once('-') {
            Some((core_text, prerelease_text)) => (core_text, Some(prerelease_text)),
            None => (text, None),
        };
        let numbers: Vec<u64> = core_text
            .split('.')
            .map(|part| parse_number(part).ok_or_else(|| invalid("expected MAJOR.MINOR.PATCH")))
            .collect::<Result<_, _>>()?;
        let core: [u64; 3] = numbers
            .try_into()
            .map_err(|_| invalid("expected MAJOR.MINOR.PATCH"))?;
        let prerelease = match prerelease_text {
            None => Vec::new(),
            Some(prerelease_text) => prerelease_text
                .split('.')
                .map(|part| identifier(part).ok_or_else(|| invalid("bad pre-release part")))
                .collect::<Result<_, _>>()?,
        };
        Ok(Self { core, prerelease })
    }

    /// `None` for a shape no release channel publishes (a nightly `-dev.N`, a release
    /// candidate): such a version is never offered and never checks for updates.
    pub(super) fn channel(&self) -> Option<Channel> {
        match self.prerelease.as_slice() {
            [] => Some(Channel::Stable),
            [Identifier::Text(name), Identifier::Numeric(_)] if name == "alpha" => {
                Some(Channel::Alpha)
            }
            [Identifier::Text(name), Identifier::Numeric(_)] if name == "beta" => {
                Some(Channel::Beta)
            }
            _ => None,
        }
    }
}

/// Digits only, no leading zero, within `u64`.
fn parse_number(part: &str) -> Option<u64> {
    let is_plain = !part.is_empty()
        && part.bytes().all(|byte| byte.is_ascii_digit())
        && (part == "0" || !part.starts_with('0'));
    if is_plain { part.parse().ok() } else { None }
}

fn identifier(part: &str) -> Option<Identifier> {
    if part.bytes().all(|byte| byte.is_ascii_digit()) {
        return parse_number(part).map(Identifier::Numeric);
    }
    let is_text = part
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    is_text.then(|| Identifier::Text(part.to_owned()))
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core.cmp(&other.core).then_with(|| {
            match (self.prerelease.is_empty(), other.prerelease.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.prerelease.cmp(&other.prerelease),
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [major, minor, patch] = self.core;
        write!(formatter, "{major}.{minor}.{patch}")?;
        for (position, part) in self.prerelease.iter().enumerate() {
            let separator = if position == 0 { '-' } else { '.' };
            match part {
                Identifier::Numeric(number) => write!(formatter, "{separator}{number}")?,
                Identifier::Text(text) => write!(formatter, "{separator}{text}")?,
            }
        }
        Ok(())
    }
}

/// What this installation is, read from its bundle manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Installed {
    pub(super) version: Version,
    pub(super) channel: Channel,
    pub(super) platform: String,
}

impl Installed {
    pub(super) fn from_manifest(manifest: &str) -> Result<Self, String> {
        let manifest: Value =
            serde_json::from_str(manifest).map_err(|error| format!("not JSON: {error}"))?;
        let version_text =
            text_field(&manifest, "version").ok_or_else(|| "has no \"version\" text".to_owned())?;
        let version = Version::parse(version_text)?;
        let channel = version.channel().ok_or_else(|| {
            format!(
                "{version} is not an alpha, beta or stable release, so it has no update channel"
            )
        })?;
        let platform = text_field(&manifest, "platform")
            .ok_or_else(|| "has no \"platform\" text".to_owned())?;
        // The platform names a download in printed shell commands: only a file-name word.
        let is_file_name_word = !platform.is_empty()
            && platform
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        if !is_file_name_word {
            return Err(format!("platform {platform:?} is not a platform name"));
        }
        Ok(Self {
            version,
            channel,
            platform: platform.to_owned(),
        })
    }

    /// `torlek-vVERSION-PLATFORM`: the archive's name and its unpacked directory's.
    pub(super) fn archive_name(&self, version: &Version) -> String {
        format!("torlek-v{version}-{}", self.platform)
    }
}

fn text_field<'a>(object: &'a Value, name: &str) -> Option<&'a str> {
    object.get(name)?.as_str()
}

/// A published release this installation may be offered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Offer {
    pub(super) version: Version,
    pub(super) notes: String,
}

impl Offer {
    pub(super) fn tag(&self) -> String {
        format!("v{}", self.version)
    }

    /// Built here, never taken from the release list: nothing the list says reaches the
    /// terminal as a link.
    pub(super) fn page_url(&self) -> String {
        format!("{}/releases/tag/{}", super::REPOSITORY_URL, self.tag())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Latest {
    UpToDate,
    Available(Offer),
    /// A newer release of this channel exists, but publishes no archive for this platform.
    NoArchiveForPlatform(Version),
}

/// The newest release of `releases_json` (GitHub's list-releases answer) that is newer than
/// `installed`, on its channel or a later one, not a draft, and has this platform's archive
/// and the checksums beside it. A release that does not read as one is skipped: it cannot be
/// offered. A list in which nothing reads is an error, since the format has probably changed.
pub(super) fn latest_release(releases_json: &str, installed: &Installed) -> Result<Latest, String> {
    let list: Value = serde_json::from_str(releases_json)
        .map_err(|error| format!("the release list is not JSON: {error}"))?;
    let entries = list
        .as_array()
        .ok_or_else(|| "the release list is not a JSON array".to_owned())?;
    let readable: Vec<ReadRelease> = entries.iter().filter_map(read_release).collect();
    if !entries.is_empty() && readable.is_empty() {
        return Err(format!(
            "none of the {} releases listed could be read",
            entries.len()
        ));
    }
    let mut newer: Vec<&ReadRelease> = readable
        .iter()
        .filter(|release| {
            !release.is_draft
                && release.version > installed.version
                && release
                    .version
                    .channel()
                    .is_some_and(|channel| channel >= installed.channel)
        })
        .collect();
    newer.sort_by(|left, right| right.version.cmp(&left.version));
    let Some(newest_published) = newer.first() else {
        return Ok(Latest::UpToDate);
    };
    let installable = newer.iter().find(|release| {
        let archive_file = format!("{}.tar.gz", installed.archive_name(&release.version));
        release.has_asset(&archive_file) && release.has_asset(CHECKSUMS_ASSET)
    });
    Ok(match installable {
        Some(release) => Latest::Available(Offer {
            version: release.version.clone(),
            notes: release.notes.clone(),
        }),
        None => Latest::NoArchiveForPlatform(newest_published.version.clone()),
    })
}

struct ReadRelease {
    is_draft: bool,
    version: Version,
    notes: String,
    asset_names: Vec<String>,
}

impl ReadRelease {
    fn has_asset(&self, name: &str) -> bool {
        self.asset_names.iter().any(|asset| asset == name)
    }
}

fn read_release(entry: &Value) -> Option<ReadRelease> {
    let is_draft = entry.get("draft")?.as_bool()?;
    let tag = text_field(entry, "tag_name")?;
    let version = Version::parse(tag.strip_prefix('v')?).ok()?;
    let asset_names = entry
        .get("assets")?
        .as_array()?
        .iter()
        .filter_map(|asset| text_field(asset, "name").map(str::to_owned))
        .collect();
    let notes = match entry.get("body") {
        Some(Value::String(body)) => printable_excerpt(body),
        Some(Value::Null) | None => String::new(),
        Some(_) => return None,
    };
    Some(ReadRelease {
        is_draft,
        version,
        notes,
        asset_names,
    })
}

/// Release notes are text from outside: control characters other than the newline are
/// dropped so they cannot drive the terminal, invisible formatting characters so they cannot
/// reorder what is shown, and the length is bounded.
fn printable_excerpt(notes: &str) -> String {
    notes
        .chars()
        .filter(|character| {
            *character == '\n' || !(character.is_control() || is_invisible_format(*character))
        })
        .take(NOTES_LIMIT)
        .collect()
}

/// Zero-width and bidirectional formatting characters (Unicode category Cf, the ones that
/// can hide or reverse text).
fn is_invisible_format(character: char) -> bool {
    matches!(
        character,
        '\u{061C}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
    )
}
