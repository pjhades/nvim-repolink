pub enum GitService {
    GitHub,
    SourceHut,
}

pub struct LineRange(pub usize, pub usize);

impl LineRange {
    fn linerange_for(&self, gs: &GitService) -> String {
        match (gs, self.0, self.1)  {
            (GitService::GitHub, a, b) if a == b => format!("#L{a}"),
            (GitService::GitHub, a, b) => format!("#L{a}-{b}"),
            /* SourceHut does not have multiline select at the time of writing. */
            (GitService::SourceHut, a, _) => format!("#L{a}"),
        }
    }
}

impl std::fmt::Display for LineRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match (self.0, self.1) {
            (begin, end) if begin == end => write!(f, "#L{begin}"),
            (begin, end) => write!(f, "#L{begin}-L{end}"),
        }
    }
}

// this is intended to build upon static strings.
pub struct Data<'a> {
    pub project: &'a str,
    pub owner: &'a str,
    pub path: &'a str,
    pub branch_or_tag_name: Option<String>,
    pub hash: Option<String>,
    pub line_range: &'a Option<LineRange>,
    pub service: GitService,
}

pub struct GitHub {}
impl GitHub {
    /* format examples:
     * https://github.com/pjhades/nvim-repolink/blob/master/src/lib.rs
     * https://github.com/psyomn/music/blob/feature/faim-ost/faim-ost/main-theme.ly
     * https://github.com/psyomn/zig-getopt/blob/v1.0.1-fake/getopt.zig */
    pub const HOST: &'static str = "github.com";
    pub fn project_url(d: &Data) -> String {
        let project = d.project;
        let owner = d.owner;
        let host = GitHub::HOST;
        format!("https://{host}/{owner}/{project}")
    }

    pub fn service_path(d: &Data) -> String {
        let path = d.path;

        if let Some(middle) = d.branch_or_tag_name.as_ref() {
            let mut ret= format!("/blob/{middle}/{path}");

            if let Some(range) = d.line_range.as_ref() {
                ret.push_str(range.linerange_for(&d.service).as_str());
            }

            return ret;
        }

        if let Some(hash) = d.hash.as_ref() {
            return format!("/commit/{hash}");
        }

        // TODO: this might not be the way to do things.
        panic!("unreachable");
    }
}

struct SourceHut {}
impl SourceHut {
    /* format examples:
     * [base-url][owner][project]/tree/[branch or tag]/item/[path]
     *      https://git.sr.ht/~psyomn/zig-postcard/tree/master/item/src/post.zig
     *      https://git.sr.ht/~psyomn/zig-postcard/commit/535309acbc07a8f745b6c1c91b87cff220913149
     *      https://git.sr.ht/~psyomn/ecophagy/tree/feature/planner/item/planner/errors.go
     *      https://git.sr.ht/~psyomn/ecophagy/tree/feature/planner/item/planner/server.go#L15
     *      https://git.sr.ht/~psyomn/oui-zig/tree/1.0.0/item/src/main.zig#L16
     *      https://git.sr.ht/~psyomn/oui-zig/tree/1.0.0/item/src/main.zig */
    const HOST: &'static str = "git.sr.ht";

    pub fn project_url(d: &Data) -> String {
        let project = d.project;
        let owner = d.owner;
        let host = SourceHut::HOST;
        /* note: sourcehut has ~user for the owner field.  This information is codified in the
         * .git/config file */
        format!("https://{host}/{owner}/{project}")
    }

    pub fn service_path(d: &Data) -> String {
        let path = d.path;

        if let Some(middle) = d.branch_or_tag_name.as_ref() {
            let mut ret= format!("/tree/{middle}/item/{path}");

            if let Some(range) = d.line_range.as_ref() {
                ret.push_str(range.linerange_for(&d.service).as_str());
            }

            return ret;
        }

        if let Some(hash) = d.hash.as_ref() {
            return format!("/commit/{hash}");
        }

        // TODO: this might not be the way to do things.
        panic!("unreachable");
    }
}

pub fn service_for(host: &str) -> Option<GitService> {
    match host {
        GitHub::HOST => Some(GitService::GitHub),
        SourceHut::HOST => Some(GitService::SourceHut),
        _ => None,
    }
}

pub fn project_url_from(d: &Data) -> String {
    match &d.service {
        GitService::GitHub => GitHub::project_url(d),
        GitService::SourceHut => SourceHut::project_url(d),
    }
}

pub fn service_path_from(d: &Data) -> String {
    match &d.service {
        GitService::GitHub => GitHub::service_path(d),
        GitService::SourceHut => SourceHut::service_path(d),
    }
}
