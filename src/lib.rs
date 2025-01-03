use anyhow::{anyhow, Error};
use git2::{BranchType, ErrorCode, Oid, Remote, Repository};
use nvim_oxi::api::opts::CreateCommandOpts;
use nvim_oxi::api::types::{CommandArgs, CommandNArgs, CommandRange};
use nvim_oxi::{api, print};

#[nvim_oxi::plugin]
fn nvim_repolink() -> Result<(), Error> {
    let opts = CreateCommandOpts::builder()
        .bang(true)
        .nargs(CommandNArgs::Zero)
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

fn split_shorthand(shorthand: &str) -> (&str, &str) {
    let parts = shorthand.split('/').collect::<Vec<&str>>();
    (parts[0], parts[1])
}

// https://github.com/pjhades/jujube/blob/master/kernel/src/memory/heap.rs#L5
//         ---------- remote url
//                    -------------- remote url
//                                        ------ remote branch
//                                               ------------------------- path, relative to repo root
//                                                                         -- nvim
fn generate_repolink(args: CommandArgs) -> Result<(), Error> {
    let path = api::get_current_buf()
        .get_name()?
        .into_os_string()
        .into_string()
        .unwrap_or_else(|s| s.as_os_str().to_string_lossy().to_string());

    let range = if args.range == 0 {
        None
    } else {
        Some((args.line1, args.line2))
    };

    let repo = Repository::discover(std::env::current_dir()?)?;
    let head = repo.head()?;

    let remote = if head.is_branch() {
        let name = std::str::from_utf8(head.shorthand_bytes())?;
        let branch = repo.find_branch(name, BranchType::Local)?;

        match branch.upstream() {
            Ok(upstream) => {
                let shorthand = std::str::from_utf8(upstream.name_bytes()?)?;
                let (remote, _) = split_shorthand(shorthand);
                repo.find_remote(remote)?
            }
            Err(e) if e.code() == ErrorCode::NotFound => {
                // Current branch doesn't track any remote ones. Try to search for
                // its tip commit in all the remote references.
                let commit = head.peel_to_commit()?;
                match locate_commit(&repo, commit.id())? {
                    Some(remote) => remote,
                    None => {
                        return Err(anyhow!("Cannot find remote matching current branch"));
                    }
                }
            }
            Err(e) => {
                return Err(Error::from(e));
            }
        }
    } else if repo.head_detached()? || head.is_tag() {
        let commit = head.peel_to_commit()?;
        match locate_commit(&repo, commit.id())? {
            Some(remote) => remote,
            None => {
                return Err(anyhow!(
                    "Cannot find remote matching current detached HEAD or tag"
                ))
            }
        }
    } else {
        return Err(anyhow!("HEAD is neither a branch nor a tag"));
    };

    print!("{}", remote.url().unwrap());

    Ok(())
}

fn locate_commit(repo: &Repository, hash: Oid) -> Result<Option<Remote<'_>>, Error> {
    for r in repo.references()? {
        if r.is_err() {
            continue;
        }

        let r = r.unwrap();
        if r.peel_to_commit()?.id() != hash {
            continue;
        }
        if r.is_remote() {
            let shorthand = std::str::from_utf8(r.shorthand_bytes())?;
            let (remote, branch) = split_shorthand(shorthand);
            if branch == "HEAD" {
                continue;
            }
            return repo
                .find_remote(remote)
                .map(Some)
                .map_err(|e| Error::from(e));
        }
    }

    Ok(None)
}
