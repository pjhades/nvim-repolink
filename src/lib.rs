use git2::{BranchType, ErrorCode, Reference, Repository};
use git_url_parse::GitUrl;
use nvim_oxi::api::opts::CreateCommandOpts;
use nvim_oxi::api::types::{CommandArgs, CommandNArgs, CommandRange};
use nvim_oxi::{api, print};
use thiserror::Error;

#[derive(Error, Debug)]
enum PluginError {
    #[error("Invalid current working directory: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid UTF-8 in {0}")]
    Utf8(&'static str),

    #[error("Nvim API error: {0}")]
    NvimApi(#[from] api::Error),

    #[error("Cannot figure out relative path of current buffer")]
    RelativePath(#[from] std::path::StripPrefixError),

    #[error("Git error: {0}")]
    Git(#[from] git2::Error),

    #[error("Parsing Git URL: {0}")]
    GitUrlParse(#[from] git_url_parse::GitUrlParseError),

    #[error("HEAD is not a branch, a tag or a commit")]
    InvalidHeadType,

    #[error("Repository is bare")]
    BareRepository,

    #[error("Missing Git hosting site")]
    MissingGitHostingSite,

    #[error("Missing repository owner")]
    MissingRepositoryOwner,

    #[error("Unsupported Git hosting site")]
    UnsupportedGitHostingSite,
}

struct LineRange(usize, usize);

impl std::fmt::Display for LineRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match (self.0, self.1) {
            (begin, end) if begin == end => write!(f, "#L{begin}"),
            (begin, end) => write!(f, "#L{begin}-L{end}"),
        }
    }
}

#[derive(Debug, PartialEq)]
enum GitObject {
    Branch(String),
    Tag(String),
    Commit(String),
}

#[nvim_oxi::plugin]
fn nvim_repolink() -> Result<(), PluginError> {
    let opts = CreateCommandOpts::builder()
        .bang(true)
        .nargs(CommandNArgs::ZeroOrOne)
        .range(CommandRange::Count(0))
        .build();

    api::create_user_command(
        "Repolink",
        |args| {
            if let Err(e) = generate_repolink(args) {
                api::err_writeln(format!("{e}").as_str());
            }
        },
        &opts,
    )?;

    Ok(())
}

fn generate_repolink(args: CommandArgs) -> Result<(), PluginError> {
    let repo = Repository::discover(std::env::current_dir()?)?;
    let remote_name = args.args.unwrap_or("origin".to_string());
    let remote = repo.find_remote(&remote_name)?;
    let url = GitUrl::parse(
        std::str::from_utf8(remote.url_bytes()).map_err(|_| PluginError::Utf8("remote URL"))?,
    )?;

    let head_obj = figure_out_git_head(&repo, &remote_name)?;

    let repo_path = repo.workdir().ok_or(PluginError::BareRepository)?;
    let file_path = api::get_current_buf().get_name()?;
    let rel_path = file_path
        .strip_prefix(repo_path)?
        .to_path_buf()
        .into_os_string()
        .into_string()
        .unwrap_or_else(|s| s.as_os_str().to_string_lossy().to_string());

    let range = if args.range == 0 {
        None
    } else {
        Some(LineRange(args.line1, args.line2))
    };

    print!("{}", make_link(url, head_obj, rel_path, range)?);

    Ok(())
}

fn figure_out_git_head(repo: &Repository, remote_name: &str) -> Result<GitObject, PluginError> {
    let head = repo.head()?;

    if head.is_note() || head.is_tag() || head.is_remote() {
        return Err(PluginError::InvalidHeadType);
    }

    let head_obj = if repo.head_detached()? {
        search_references(&repo, |r| {
            if !r.is_tag() {
                return None;
            }
            std::str::from_utf8(r.shorthand_bytes())
                .ok()
                .map(|s| GitObject::Tag(s.to_string()))
        })?
        .or_else(|| {
            head.peel_to_commit()
                .ok()
                .map(|commit| GitObject::Commit(commit.id().to_string()))
        })
    } else if head.is_branch() {
        get_remote_branch(&repo, remote_name)?
    } else {
        None
    };

    head_obj.ok_or(PluginError::InvalidHeadType)
}

fn make_link(
    url: GitUrl,
    head_obj: GitObject,
    path: String,
    range: Option<LineRange>,
) -> Result<String, PluginError> {
    let project = project_name(&url);
    let host = url.host.ok_or(PluginError::MissingGitHostingSite)?;
    let owner = url.owner.ok_or(PluginError::MissingRepositoryOwner)?;
    let mut link = String::new();

    match host {
        h if h == "github.com" => link.push_str(format!("https://{h}/{owner}/{project}").as_str()),
        _ => return Err(PluginError::UnsupportedGitHostingSite),
    }

    match head_obj {
        GitObject::Branch(name) | GitObject::Tag(name) => {
            link.push_str(format!("/blob/{name}/{path}").as_str());
            if let Some(range) = range {
                link.push_str(format!("{range}").as_str());
            }
        }
        GitObject::Commit(hash) => link.push_str(format!("/commit/{hash}").as_str()),
    }

    Ok(link)
}

fn get_remote_branch(
    repo: &Repository,
    wanted_remote: &str,
) -> Result<Option<GitObject>, PluginError> {
    let head = repo.head()?;
    let name =
        std::str::from_utf8(head.shorthand_bytes()).map_err(|_| PluginError::Utf8("HEAD"))?;
    let branch = repo.find_branch(name, BranchType::Local)?;

    match branch.upstream() {
        Ok(upstream) => {
            let shorthand = std::str::from_utf8(upstream.name_bytes()?)
                .map_err(|_| PluginError::Utf8("remote branch"))?;
            let (_, branch) = split_shorthand(shorthand);
            Ok(Some(GitObject::Branch(branch.to_string())))
        }
        Err(e) if e.code() == ErrorCode::NotFound => search_references(&repo, |r| {
            if !r.is_remote() {
                return None;
            }
            std::str::from_utf8(r.shorthand_bytes())
                .ok()
                .and_then(|shorthand| {
                    let (remote, branch) = split_shorthand(shorthand);
                    if remote == wanted_remote && branch != "HEAD" {
                        Some(GitObject::Branch(branch.to_string()))
                    } else {
                        None
                    }
                })
        }),
        Err(e) => Err(e.into()),
    }
}

fn search_references(
    repo: &Repository,
    f: impl Fn(Reference<'_>) -> Option<GitObject>,
) -> Result<Option<GitObject>, PluginError> {
    let head = repo.head()?;
    let hash = head.peel_to_commit()?.id();
    let ret = repo.references()?.find_map(|r| {
        if r.is_err() {
            return None;
        }
        let r = r.unwrap();
        match r.peel_to_commit() {
            Err(_) => None,
            Ok(commit) if commit.id() != hash => None,
            _ => f(r),
        }
    });
    Ok(ret)
}

fn split_shorthand(shorthand: &str) -> (&str, &str) {
    let parts = shorthand.split('/').collect::<Vec<&str>>();
    (parts[0], parts[1])
}

fn project_name(url: &GitUrl) -> String {
    let parts = url.path.as_str().split('/').collect::<Vec<&str>>();
    let name = parts.iter().last().unwrap();
    if url.git_suffix {
        name.strip_suffix(".git").unwrap().to_string()
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{Branch, ObjectType, Oid, Repository, Signature};
    use std::fs::File;
    use std::io::Write;
    use std::path::Path;
    use tempdir::TempDir;

    const FILENAME: &'static str = "txt";
    const SONNET: &'static str = "\
Shall I compare thee to a summer’s day?
Thou art more lovely and more temperate:
Rough winds do shake the darling buds of May,
And summer’s lease hath all too short a date;
Sometime too hot the eye of heaven shines,
And often is his gold complexion dimm'd;
And every fair from fair sometime declines,
By chance or nature’s changing course untrimm'd;
But thy eternal summer shall not fade,
Nor lose possession of that fair thou ow’st;
Nor shall death brag thou wander’st in his shade,
When in eternal lines to time thou grow’st:
   So long as men can breathe or eyes can see,
   So long lives this, and this gives life to thee.
";

    struct MockRepository {
        path: TempDir,
        repo: Repository,
    }

    impl MockRepository {
        fn new() -> Self {
            let path = TempDir::new("mock-repo").unwrap();
            println!("{path:?}");
            let repo = Repository::init(&path).unwrap();
            Self { path, repo }
        }

        fn git_remote_add(&self, name: &str, url: &str) {
            self.repo.remote(name, url).unwrap();
        }

        fn git_add<P: AsRef<Path>>(&self, path: P) {
            let mut index = self.repo.index().unwrap();
            index.add_path(path.as_ref()).unwrap();
            index.write().unwrap();
        }

        fn git_commit(&self, msg: &str) -> Oid {
            let mut index = self.repo.index().unwrap();
            let tree_id = index.write_tree().unwrap();
            let tree = self.repo.find_tree(tree_id).unwrap();
            let sig = Signature::now("somebody", "somebody@somewhere.com").unwrap();
            if let Ok(head) = self.repo.head() {
                let commit = head.peel_to_commit().unwrap();
                self.repo
                    .commit(Some("HEAD"), &sig, &sig, msg, &tree, &[&commit])
                    .unwrap()
            } else {
                self.repo
                    .commit(Some("HEAD"), &sig, &sig, msg, &tree, &[])
                    .unwrap()
            }
        }

        fn git_branch(&self, name: &str) -> Option<Branch<'_>> {
            self.repo.head().ok().map(|head| {
                let commit = head.peel_to_commit().unwrap();
                self.repo.branch(name, &commit, true).unwrap()
            })
        }

        fn git_tag(&self, name: &str, msg: &str, commit_id: Oid) {
            let commit = self
                .repo
                .find_object(commit_id, Some(ObjectType::Commit))
                .unwrap();
            let sig = Signature::now("somebody", "somebody@somewhere.com").unwrap();
            self.repo.tag(name, &commit, &sig, msg, true).unwrap();
        }

        fn setup(&self) {
            self.git_remote_add("origin", "git@github.com:user/repo.git");
            self.git_remote_add("https", "https://github.com/user/repo.git");

            let mut file = File::create(self.path.path().join(FILENAME)).unwrap();
            file.write(SONNET.as_bytes()).unwrap();

            // Make a local commit.
            self.git_add(FILENAME);
            let commit_id = self.git_commit("Add sonnet");

            // Add a remote reference to mock a remote branch.
            self.repo
                .reference("refs/remotes/origin/up", commit_id, true, "Add reference")
                .unwrap();

            // Create branch `sonnet`, tracking `origin/up`.
            let mut branch = self.git_branch("sonnet").unwrap();
            branch.set_upstream(Some("origin/up")).unwrap();

            // Make the second commit and tag it.
            file.write(b"Sonnet 18\n").unwrap();
            self.git_add(FILENAME);
            let commit_id = self.git_commit("Add title");
            self.git_tag("v1.0", "Add tag", commit_id);

            // Make the third commit.
            file.write(b"William Shakespeare\n").unwrap();
            self.git_add(FILENAME);
            self.git_commit("Add author");
        }
    }

    #[test]
    fn remote_branch() {
        let m = MockRepository::new();
        m.setup();

        let b = get_remote_branch(&m.repo, "origin");
        assert!(b.is_ok());
        assert_eq!(b.unwrap(), None);
    }
}
