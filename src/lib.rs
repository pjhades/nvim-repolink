use git2::{BranchType, ErrorCode, Reference, Repository};
use git_url_parse::GitUrl;
use nvim_oxi::api::opts::CreateCommandOpts;
use nvim_oxi::api::types::{CommandArgs, CommandNArgs, CommandRange};
use nvim_oxi::conversion::{self, FromObject, ToObject};
use nvim_oxi::serde::{Deserializer, Serializer};
use nvim_oxi::{api, lua, print, Dictionary, Function, Object};
use serde::{Deserialize, Serialize};
use thiserror::Error;

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

    #[error("Missing Git service")]
    MissingGitService,

    #[error("Missing repository owner")]
    MissingRepositoryOwner,

    #[error("Unsupported Git service: {0}")]
    UnsupportedGitService(String),
}

#[derive(Copy, Clone)]
enum GitService {
    GitHub,
    SourceHut,
}

impl GitService {
    fn new(url: &GitUrl) -> Result<Self, PluginError> {
        if url.owner.is_none() {
            return Err(PluginError::MissingRepositoryOwner);
        }
        match url.host.as_ref().map(|s| s.as_str()) {
            Some("github.com") => Ok(Self::GitHub),
            Some("git.sr.ht") => Ok(Self::SourceHut),
            Some(s) => Err(PluginError::UnsupportedGitService(s.to_string())),
            None => Err(PluginError::MissingGitService),
        }
    }
}

struct LineRange(usize, usize);

struct GitServiceUrl {
    service: GitService,
    url: GitUrl,
    obj: String,
    path: String,
    range: Option<LineRange>,
}

impl GitServiceUrl {
    fn new(
        url: GitUrl,
        obj: String,
        path: String,
        range: Option<LineRange>,
    ) -> Result<Self, PluginError> {
        Ok(Self {
            service: GitService::new(&url)?,
            url,
            obj,
            path,
            range,
        })
    }
}

impl std::fmt::Display for GitServiceUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        let path = match (self.service, &self.obj) {
            // https://github.com/<owner>/<project>/blob/<obj>/<path>
            (GitService::GitHub, obj) => format!("blob/{}/{}", obj, self.path),
            // https://git.sr.ht/<owner>/<project>/tree/<obj>/item/<path>
            (GitService::SourceHut, obj) => format!("tree/{}/item/{}", obj, self.path),
        };

        let range = match (self.service, self.range.as_ref()) {
            (_, None) => format!(""),
            // SourceHut does not have multiline highlighting at the time of writing.
            (GitService::SourceHut, Some(LineRange(a, _))) => format!("#L{a}"),
            (_, Some(LineRange(a, b))) if a == b => format!("#L{a}"),
            (_, Some(LineRange(a, b))) => format!("#L{a}-L{b}"),
        };

        write!(
            f,
            "https://{}/{}/{}/{}{}",
            self.url.host.as_ref().unwrap(),
            self.url.owner.as_ref().unwrap(),
            project_name(&self.url),
            path,
            range
        )
    }
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

    print!("{}", GitServiceUrl::new(url, head_obj, rel_path, range)?);

    Ok(())
}

fn figure_out_git_head(repo: &Repository, remote_name: &str) -> Result<String, PluginError> {
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
                .map(|s| s.to_string())
        })?
        .or_else(|| {
            head.peel_to_commit()
                .ok()
                .map(|commit| commit.id().to_string())
        })
    } else if head.is_branch() {
        get_remote_branch(&repo, remote_name)?
    } else {
        None
    };

    head_obj.ok_or(PluginError::InvalidHeadType)
}

fn get_remote_branch(
    repo: &Repository,
    wanted_remote: &str,
) -> Result<Option<String>, PluginError> {
    let head = repo.head()?;
    let name =
        std::str::from_utf8(head.shorthand_bytes()).map_err(|_| PluginError::Utf8("HEAD"))?;
    let branch = repo.find_branch(name, BranchType::Local)?;

    match branch.upstream() {
        Ok(upstream) => {
            let shorthand = std::str::from_utf8(upstream.name_bytes()?)
                .map_err(|_| PluginError::Utf8("remote branch"))?;
            let (_, branch) = split_shorthand(shorthand);
            Ok(Some(branch.to_string()))
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
                        Some(branch.to_string())
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
    f: impl Fn(Reference<'_>) -> Option<String>,
) -> Result<Option<String>, PluginError> {
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
