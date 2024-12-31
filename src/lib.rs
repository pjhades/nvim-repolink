use nvim_oxi::api::opts::CreateCommandOpts;
use nvim_oxi::api::types::{CommandArgs, CommandNArgs};
use nvim_oxi::api::{create_user_command, Buffer};
use nvim_oxi::{print, Result};

#[nvim_oxi::plugin]
fn nvim_repolink() -> Result<()> {
    let opts = CreateCommandOpts::builder()
        .bang(true)
        .nargs(CommandNArgs::Zero)
        .build();

    create_user_command("Repolink", generate_repolink, &opts)?;

    Ok(())
}

fn generate_repolink(_args: CommandArgs) -> Result<()> {
    let buf = Buffer::current();
    let path = buf
        .get_name()?
        .into_os_string()
        .into_string()
        .unwrap_or_else(|s| s.as_os_str().to_string_lossy().to_string());
    print!("{}", path);
    Ok(())
}
