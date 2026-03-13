use {
    crate::managed_worktree,
    arbor_daemon_client::{IssueDto, IssueListResponse, IssueSourceDto},
    serde::Deserialize,
    std::{path::Path, time::Duration},
};

const ISSUE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const ISSUE_PAGE_SIZE: usize = 100;

pub(crate) trait RepositoryIssueProvider: Send + Sync {
    fn resolve_source(
        &self,
        repo_root: &Path,
        origin_remote_url: &str,
    ) -> Option<ResolvedIssueSource>;
    fn list_issues(&self, source: &ResolvedIssueSource) -> Result<Vec<IssueDto>, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedIssueSource {
    provider: String,
    label: String,
    repository: String,
    url: Option<String>,
    api_base_url: String,
}

pub(crate) struct RepositoryIssueService {
    providers: Vec<Box<dyn RepositoryIssueProvider>>,
}

impl RepositoryIssueService {
    pub(crate) fn new(providers: Vec<Box<dyn RepositoryIssueProvider>>) -> Self {
        Self { providers }
    }

    pub(crate) fn list_repository_issues(
        &self,
        repo_root: &Path,
    ) -> Result<IssueListResponse, String> {
        let Some(origin_remote_url) = origin_remote_url(repo_root)? else {
            return Ok(IssueListResponse {
                source: None,
                issues: Vec::new(),
                notice: Some("Repository has no origin remote.".to_owned()),
            });
        };

        for provider in &self.providers {
            let Some(source) = provider.resolve_source(repo_root, &origin_remote_url) else {
                continue;
            };

            let issues = provider.list_issues(&source)?;
            return Ok(IssueListResponse {
                source: Some(IssueSourceDto {
                    provider: source.provider,
                    label: source.label,
                    repository: source.repository,
                    url: source.url,
                }),
                issues,
                notice: None,
            });
        }

        Ok(IssueListResponse {
            source: None,
            issues: Vec::new(),
            notice: Some("No supported issue provider resolved from the origin remote.".to_owned()),
        })
    }
}

impl Default for RepositoryIssueService {
    fn default() -> Self {
        Self::new(vec![
            Box::new(GitHubIssueProvider),
            Box::new(GitLabIssueProvider),
        ])
    }
}

struct GitHubIssueProvider;

impl RepositoryIssueProvider for GitHubIssueProvider {
    fn resolve_source(
        &self,
        _repo_root: &Path,
        origin_remote_url: &str,
    ) -> Option<ResolvedIssueSource> {
        let repository = github_repo_slug_from_remote_url(origin_remote_url)?;
        Some(ResolvedIssueSource {
            provider: "github".to_owned(),
            label: "GitHub".to_owned(),
            repository: repository.clone(),
            url: Some(format!("https://github.com/{repository}/issues")),
            api_base_url: "https://api.github.com".to_owned(),
        })
    }

    fn list_issues(&self, source: &ResolvedIssueSource) -> Result<Vec<IssueDto>, String> {
        let (owner, repository) = source
            .repository
            .split_once('/')
            .ok_or_else(|| format!("invalid GitHub repository slug `{}`", source.repository))?;
        let token = github_access_token_from_env();
        let mut issues = Vec::new();
        let mut page = 1usize;

        loop {
            let url = format!(
                "{}/repos/{}/{}/issues?state=open&sort=updated&direction=desc&per_page={}&page={page}",
                source.api_base_url,
                percent_encode(owner),
                percent_encode(repository),
                ISSUE_PAGE_SIZE,
            );
            let mut request = ureq::get(&url)
                .header("Accept", "application/json")
                .header("User-Agent", "Arbor");
            if let Some(token) = token.as_deref() {
                request = request.header("Authorization", &format!("Bearer {token}"));
            }

            let mut response = request
                .config()
                .timeout_global(Some(ISSUE_REQUEST_TIMEOUT))
                .build()
                .call()
                .map_err(|error| format!("GitHub request failed: {error}"))?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.body_mut().read_to_string().unwrap_or_default();
                return Err(format!("GitHub returned {status}: {body}"));
            }

            let body = response
                .body_mut()
                .read_to_string()
                .map_err(|error| format!("failed to read GitHub response: {error}"))?;
            let page_items: Vec<GitHubIssuePayload> = serde_json::from_str(&body)
                .map_err(|error| format!("failed to decode GitHub issues: {error}"))?;
            let page_len = page_items.len();

            issues.extend(
                page_items
                    .into_iter()
                    .filter(|issue| issue.pull_request.is_none())
                    .map(|issue| IssueDto {
                        id: issue.number.to_string(),
                        display_id: format!("#{}", issue.number),
                        title: issue.title.clone(),
                        state: issue.state,
                        url: Some(issue.html_url),
                        suggested_worktree_name: issue_worktree_name(
                            &issue.number.to_string(),
                            &issue.title,
                        ),
                        updated_at: issue.updated_at,
                    }),
            );

            if page_len < ISSUE_PAGE_SIZE {
                break;
            }
            page += 1;
        }

        Ok(issues)
    }
}

struct GitLabIssueProvider;

impl RepositoryIssueProvider for GitLabIssueProvider {
    fn resolve_source(
        &self,
        _repo_root: &Path,
        origin_remote_url: &str,
    ) -> Option<ResolvedIssueSource> {
        let remote = parse_remote(origin_remote_url)?;
        if !remote.host.contains("gitlab") {
            return None;
        }

        let url = format!(
            "{}://{}/{}/-/issues",
            remote.scheme, remote.host, remote.path
        );
        Some(ResolvedIssueSource {
            provider: "gitlab".to_owned(),
            label: "GitLab".to_owned(),
            repository: remote.path.clone(),
            url: Some(url),
            api_base_url: format!("{}://{}/api/v4", remote.scheme, remote.host),
        })
    }

    fn list_issues(&self, source: &ResolvedIssueSource) -> Result<Vec<IssueDto>, String> {
        let token = gitlab_access_token_from_env();
        let mut issues = Vec::new();
        let mut page = 1usize;

        loop {
            let url = format!(
                "{}/projects/{}/issues?state=opened&order_by=updated_at&sort=desc&per_page={}&page={page}",
                source.api_base_url,
                percent_encode(&source.repository),
                ISSUE_PAGE_SIZE,
            );
            let mut request = ureq::get(&url)
                .header("Accept", "application/json")
                .header("User-Agent", "Arbor");
            if let Some(token) = token.as_deref() {
                request = request.header("PRIVATE-TOKEN", token);
            }

            let mut response = request
                .config()
                .timeout_global(Some(ISSUE_REQUEST_TIMEOUT))
                .build()
                .call()
                .map_err(|error| format!("GitLab request failed: {error}"))?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.body_mut().read_to_string().unwrap_or_default();
                return Err(format!("GitLab returned {status}: {body}"));
            }

            let body = response
                .body_mut()
                .read_to_string()
                .map_err(|error| format!("failed to read GitLab response: {error}"))?;
            let page_items: Vec<GitLabIssuePayload> = serde_json::from_str(&body)
                .map_err(|error| format!("failed to decode GitLab issues: {error}"))?;
            let page_len = page_items.len();

            issues.extend(page_items.into_iter().map(|issue| IssueDto {
                id: issue.id.to_string(),
                display_id: format!("#{}", issue.iid),
                title: issue.title.clone(),
                state: issue.state,
                url: issue.web_url,
                suggested_worktree_name: issue_worktree_name(&issue.iid.to_string(), &issue.title),
                updated_at: issue.updated_at,
            }));

            if page_len < ISSUE_PAGE_SIZE {
                break;
            }
            page += 1;
        }

        Ok(issues)
    }
}

#[derive(Debug, Deserialize)]
struct GitHubIssuePayload {
    number: u64,
    title: String,
    html_url: String,
    state: String,
    updated_at: Option<String>,
    pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GitLabIssuePayload {
    id: u64,
    iid: u64,
    title: String,
    state: String,
    web_url: Option<String>,
    updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteSpec {
    scheme: String,
    host: String,
    path: String,
}

fn origin_remote_url(repo_root: &Path) -> Result<Option<String>, String> {
    let repo = gix::open(repo_root).map_err(|error| {
        format!(
            "failed to open repository `{}`: {error}",
            repo_root.display()
        )
    })?;
    let remote = match repo.find_remote("origin") {
        Ok(remote) => remote,
        Err(_) => return Ok(None),
    };
    let Some(url) = remote.url(gix::remote::Direction::Fetch) else {
        return Ok(None);
    };
    let url = url.to_bstring().to_string();
    if url.is_empty() {
        Ok(None)
    } else {
        Ok(Some(url))
    }
}

fn github_repo_slug_from_remote_url(remote_url: &str) -> Option<String> {
    let path = remote_url
        .strip_prefix("git@github.com:")
        .or_else(|| remote_url.strip_prefix("https://github.com/"))
        .or_else(|| remote_url.strip_prefix("http://github.com/"))
        .or_else(|| remote_url.strip_prefix("ssh://git@github.com/"))?;

    let normalized = path.trim_end_matches('/');
    let repository = normalized.strip_suffix(".git").unwrap_or(normalized);
    let (owner, repo_name) = repository.split_once('/')?;
    if owner.is_empty() || repo_name.is_empty() {
        return None;
    }
    Some(format!("{owner}/{repo_name}"))
}

fn parse_remote(remote_url: &str) -> Option<RemoteSpec> {
    let trimmed = remote_url.trim();

    if let Some(rest) = trimmed.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        return Some(RemoteSpec {
            scheme: "https".to_owned(),
            host: host.to_owned(),
            path: normalize_remote_path(path)?,
        });
    }

    if let Some(rest) = trimmed.strip_prefix("ssh://") {
        let (authority, path) = rest.split_once('/')?;
        let host = authority
            .rsplit_once('@')
            .map(|(_, host)| host)
            .unwrap_or(authority);
        return Some(RemoteSpec {
            scheme: "https".to_owned(),
            host: host.to_owned(),
            path: normalize_remote_path(path)?,
        });
    }

    if let Some(rest) = trimmed.strip_prefix("https://") {
        let (host, path) = rest.split_once('/')?;
        return Some(RemoteSpec {
            scheme: "https".to_owned(),
            host: host.to_owned(),
            path: normalize_remote_path(path)?,
        });
    }

    if let Some(rest) = trimmed.strip_prefix("http://") {
        let (host, path) = rest.split_once('/')?;
        return Some(RemoteSpec {
            scheme: "http".to_owned(),
            host: host.to_owned(),
            path: normalize_remote_path(path)?,
        });
    }

    None
}

fn normalize_remote_path(path: &str) -> Option<String> {
    let normalized = path.trim_end_matches('/');
    let path = normalized.strip_suffix(".git").unwrap_or(normalized);
    if path.is_empty() {
        None
    } else {
        Some(path.to_owned())
    }
}

fn issue_worktree_name(reference: &str, title: &str) -> String {
    let reference_slug = managed_worktree::sanitize_worktree_name(reference);
    let title_slug = managed_worktree::sanitize_worktree_name(title);

    let base_reference = if reference_slug.is_empty() {
        "issue".to_owned()
    } else if reference_slug
        .chars()
        .all(|character| character.is_ascii_digit() || character == '-')
    {
        format!("issue-{reference_slug}")
    } else {
        reference_slug
    };

    if title_slug.is_empty() {
        base_reference
    } else {
        format!("{base_reference}-{title_slug}")
    }
}

fn github_access_token_from_env() -> Option<String> {
    std::env::var("GITHUB_TOKEN")
        .ok()
        .and_then(|value| non_empty_trimmed_str(&value).map(str::to_owned))
}

fn gitlab_access_token_from_env() -> Option<String> {
    std::env::var("GITLAB_TOKEN")
        .ok()
        .or_else(|| std::env::var("ARBOR_GITLAB_TOKEN").ok())
        .and_then(|value| non_empty_trimmed_str(&value).map(str::to_owned))
}

fn non_empty_trimmed_str(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn percent_encode(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            },
            _ => {
                result.push('%');
                result.push_str(&format!("{byte:02X}"));
            },
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_repo_slug_from_remote_url_supports_common_formats() {
        assert_eq!(
            github_repo_slug_from_remote_url("git@github.com:penso/arbor.git"),
            Some("penso/arbor".to_owned())
        );
        assert_eq!(
            github_repo_slug_from_remote_url("https://github.com/penso/arbor"),
            Some("penso/arbor".to_owned())
        );
    }

    #[test]
    fn parse_remote_handles_gitlab_urls() {
        assert_eq!(
            parse_remote("git@gitlab.com:group/subgroup/arbor.git"),
            Some(RemoteSpec {
                scheme: "https".to_owned(),
                host: "gitlab.com".to_owned(),
                path: "group/subgroup/arbor".to_owned(),
            })
        );
        assert_eq!(
            parse_remote("https://gitlab.example.com/group/arbor.git"),
            Some(RemoteSpec {
                scheme: "https".to_owned(),
                host: "gitlab.example.com".to_owned(),
                path: "group/arbor".to_owned(),
            })
        );
    }

    #[test]
    fn issue_worktree_name_uses_issue_prefix_for_numeric_references() {
        assert_eq!(
            issue_worktree_name("42", "Fix auth callback race"),
            "issue-42-fix-auth-callback-race"
        );
        assert_eq!(issue_worktree_name("42", ""), "issue-42");
    }
}
