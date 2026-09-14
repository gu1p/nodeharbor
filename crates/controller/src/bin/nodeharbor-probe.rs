use clap::Parser;
use std::net::SocketAddr;
#[derive(Parser)]
#[command(version = nodeharbor_core::build_version(), about = "NodeHarbor DNS and overlay network probe")]
struct Args {
    #[arg(long, env = "NODE_NAME")]
    node_name: String,
    #[arg(long, default_value = "0.0.0.0:8091")]
    listen: SocketAddr,
    #[arg(long, default_value = "kubernetes.default.svc.cluster.local.")]
    dns_name: String,
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    axum::serve(
        listener,
        nodeharbor_controller::probe_router(args.node_name, args.dns_name),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_cluster_dns_query_is_absolute_and_avoids_search_suffixes() {
        let args =
            Args::try_parse_from(["nodeharbor-probe", "--node-name", "test-worker"]).unwrap();
        assert_eq!(args.dns_name, "kubernetes.default.svc.cluster.local.");
    }
}
