use anyhow::{anyhow, Error};
use git2::{BranchType, ErrorCode, Reference, Repository};
use git_url_parse::GitUrl;
use nvim_oxi::api::opts::CreateCommandOpts;
use nvim_oxi::api::types::{CommandArgs, CommandNArgs, CommandRange};
use nvim_oxi::{api, print};

#[derive(Debug)]
struct Utf8Error(&'static str);

impl std::fmt::Display for Utf8Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "invalid utf-8 in {}", self.0)
    }
}

impl std::error::Error for Utf8Error {}

struct LineRange(usize, usize);

impl std::fmt::Display for LineRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match (self.0, self.1) {
            (begin, end) if begin == end => write!(f, "#L{begin}"),
            (begin, end) => write!(f, "#L{begin}-L{end}"),
        }
    }
}

enum GitObject {
    Branch(String),
    Tag(String),
    Commit(String),
}

#[nvim_oxi::plugin]
fn nvim_repolink() -> Result<(), Error> {
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

fn generate_repolink(args: CommandArgs) -> Result<(), Error> {
    let repo = Repository::discover(std::env::current_dir()?)?;
    let remote_name = args.args.unwrap_or("origin".to_string());
    let remote = repo.find_remote(&remote_name)?;
    let head = repo.head()?;

    if head.is_note() || head.is_tag() || head.is_remote() {
        return Err(anyhow!("head points directly to a note, tag or remote"));
    }

    // Figure out what HEAD is: a branch, a tag, or a commit.
    let gitobj = if repo.head_detached()? {
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
        get_remote_branch(&repo, &remote_name)?
    } else {
        None
    }
    .ok_or(anyhow!("head is not a branch, a tag or a commit"))?;

    let repo_path = repo.workdir().ok_or(anyhow!("repository is bare"))?;
    let file_path = api::get_current_buf().get_name()?;
    let rel_path = file_path
        .strip_prefix(repo_path)
        .map_err(|e| anyhow!("cannot figure out relative path of current buffer: {e}"))?
        .to_path_buf()
        .into_os_string()
        .into_string()
        .unwrap_or_else(|s| s.as_os_str().to_string_lossy().to_string());

    let range = if args.range == 0 {
        None
    } else {
        Some(LineRange(args.line1, args.line2))
    };

    let url = GitUrl::parse(std::str::from_utf8(remote.url_bytes())?)?;

    print!("{}", make_link(url, gitobj, rel_path, range)?);

    Ok(())
}

fn make_link(
    url: GitUrl,
    gitobj: GitObject,
    path: String,
    range: Option<LineRange>,
) -> Result<String, Error> {
    let project = project_name(&url);
    let host = url.host.ok_or(anyhow!("unknown Git hosting site"))?;
    let owner = url.owner.ok_or(anyhow!("unknown repository owner"))?;
    let mut link = String::new();

    match host {
        h if h == "github.com" => link.push_str(format!("https://{h}/{owner}/{project}").as_str()),
        _ => return Err(anyhow!("unknown git hosting site")),
    }

    match gitobj {
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

fn get_remote_branch(repo: &Repository, wanted_remote: &str) -> Result<Option<GitObject>, Error> {
    let head = repo.head()?;
    let name = std::str::from_utf8(head.shorthand_bytes())?;
    let branch = repo.find_branch(name, BranchType::Local)?;

    match branch.upstream() {
        Ok(upstream) => {
            let shorthand = std::str::from_utf8(upstream.name_bytes()?)?;
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
) -> Result<Option<GitObject>, Error> {
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
    parts
        .iter()
        .last()
        .unwrap()
        .strip_suffix(".git")
        .unwrap()
        .to_string()
}
