//! Reading which GitHub repository a git remote points at.

/// A github.com repository named by a git remote URL.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GitHubRemote {
    pub owner: String,
    pub name: String,
    /// Whether the remote is reached over HTTPS, where a token is the
    /// credential. Over SSH, the user's SSH key decides access instead.
    pub https: bool,
}

impl GitHubRemote {
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// The github.com repository `url` points at, or `None` for any other host.
/// Understands `https://github.com/owner/repo(.git)`, with or without a user
/// name before the host, and the SSH forms `git@github.com:owner/repo.git`
/// and `ssh://git@github.com/owner/repo.git`.
pub fn parse_github_remote(url: &str) -> Option<GitHubRemote> {
    let url = url.trim();
    let (https, path) = if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    {
        (true, host_path(rest)?)
    } else if let Some(rest) = url.strip_prefix("ssh://") {
        (false, host_path(rest)?)
    } else {
        // scp-like SSH: `[user@]github.com:owner/repo.git`
        let (host, path) = url.split_once(':')?;
        let host = host.rsplit('@').next()?;
        if !host.eq_ignore_ascii_case("github.com") {
            return None;
        }
        (false, path)
    };

    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let owner = parts.next().filter(|part| !part.is_empty())?;
    let name = parts.next().filter(|part| !part.is_empty())?;
    if parts.next().is_some() {
        return None;
    }
    Some(GitHubRemote {
        owner: owner.to_string(),
        name: name.to_string(),
        https,
    })
}

/// The path of `rest` (`[user@]host[:port]/path`) when the host is
/// github.com.
fn host_path(rest: &str) -> Option<&str> {
    let (authority, path) = rest.split_once('/')?;
    let host = authority.rsplit('@').next()?;
    let host = host.split(':').next()?;
    host.eq_ignore_ascii_case("github.com").then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(owner: &str, name: &str, https: bool) -> Option<GitHubRemote> {
        Some(GitHubRemote {
            owner: owner.into(),
            name: name.into(),
            https,
        })
    }

    #[test]
    fn https_remotes() {
        assert_eq!(
            parse_github_remote("https://github.com/mentenaz/zed_Source.git"),
            remote("mentenaz", "zed_Source", true)
        );
        assert_eq!(
            parse_github_remote("https://github.com/mentenaz/zed_Source"),
            remote("mentenaz", "zed_Source", true)
        );
        assert_eq!(
            parse_github_remote("https://someone@GitHub.com/Mentenaz-Server/CreateRepoTesy.git/"),
            remote("Mentenaz-Server", "CreateRepoTesy", true)
        );
    }

    #[test]
    fn ssh_remotes() {
        assert_eq!(
            parse_github_remote("git@github.com:mentenaz/zed_Source.git"),
            remote("mentenaz", "zed_Source", false)
        );
        assert_eq!(
            parse_github_remote("ssh://git@github.com/mentenaz/zed_Source.git"),
            remote("mentenaz", "zed_Source", false)
        );
    }

    #[test]
    fn other_hosts_and_odd_paths_are_not_github_repositories() {
        assert_eq!(parse_github_remote("https://dev.azure.com/org/project/_git/repo"), None);
        assert_eq!(parse_github_remote("git@gitlab.com:owner/repo.git"), None);
        assert_eq!(parse_github_remote("https://github.com/owner"), None);
        assert_eq!(parse_github_remote("https://github.com/owner/repo/tree/main"), None);
        assert_eq!(parse_github_remote("C:/code/repo"), None);
    }
}
