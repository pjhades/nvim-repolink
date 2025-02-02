use git2::{BranchType, ErrorCode, Reference, Repository};
use git_url_parse::GitUrl;
use nvim_oxi::api::opts::CreateCommandOpts;
use nvim_oxi::api::types::{CommandArgs, CommandNArgs, CommandRange};
use nvim_oxi::conversion::{self, FromObject, ToObject};
use nvim_oxi::serde::{Deserializer, Serializer};
use nvim_oxi::{api, lua, print, Dictionary, Function, Object};
use serde::{Deserialize, Serialize};
use thiserror::Error;

mod services;
use services::LineRange;

#[derive(Serialize, Deserialize)]
struct Config {}

impl FromObject for Config {
    fn from_object(obj: Object) -> Result<Self, conversion::Error> {
        Self::deserialize(Deserializer::new(obj)).map_err(Into::into)
    }
}

impl ToObject for Config {
    fn to_object(self) -> Result<Object, conversion::Error> {
        self.serialize(Serializer::new()).map_err(Into::into)
    }
}

impl lua::Poppable for Config {
    unsafe fn pop(lstate: *mut lua::ffi::lua_State) -> Result<Self, lua::Error> {
        let obj = Object::pop(lstate)?;
        Self::from_object(obj).map_err(lua::Error::pop_error_from_err::<Self, _>)
    }
}

impl lua::Pushable for Config {
    unsafe fn push(self, lstate: *mut lua::ffi::lua_State) -> Result<std::ffi::c_int, lua::Error> {
        self.to_object()
            .map_err(lua::Error::push_error_from_err::<Self, _>)?
            .push(lstate)
    }
}

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

    #[error("Unsupported Git hosting site: {0}")]
    UnsupportedGitHostingSite(String),
}

#[derive(Debug, PartialEq)]
enum GitObject {
    Branch(String),
    Tag(String),
    Commit(String),
}

#[nvim_oxi::plugin]
fn nvim_repolink() -> Result<Dictionary, PluginError> {
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

    // This will allow Lazy to call `require(...).setup({})`, so that we won't have to ask the user
    // to manually call `require` or using `config = ...` in Lazy. Lazy dissuades the use of
    // `config`. See https://lazy.folke.io/spec.
    Ok(Dictionary::from_iter([(
        "setup",
        Object::from(Function::from_fn(|_: Config| {})),
    )]))
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

    use services::Data;

    let service = services::service_for(host.as_str());
    if service.is_none() {
        return Err(PluginError::UnsupportedGitHostingSite(host));
    }

    let mut data = Data{
        project: project.as_str(),
        owner: owner.as_str(),
        path: path.as_str(),
        service: service.unwrap(),
        line_range: &range,
        branch_or_tag_name: None,
        hash: None,
    };

    link.push_str(services::project_url_from(&data).as_str());

    match head_obj {
        GitObject::Branch(name) | GitObject::Tag(name) =>
            data.branch_or_tag_name = Some(name),
        GitObject::Commit(hash) =>
            data.hash = Some(hash),
    }

    link.push_str(services::service_path_from(&data).as_str());

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
    let (first, rest) = shorthand.split_once('/').unwrap();
    (first, rest)
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
