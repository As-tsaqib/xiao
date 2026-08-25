use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.as_slice() == ["daemon"] {
        return xiao::runtime::host::RuntimeHost::bootstrap()
            .await?
            .run()
            .await;
    }

    let code = xiao::cli::run_process(args).await;
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}
