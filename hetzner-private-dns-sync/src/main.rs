use std::{
    collections::HashSet,
    fmt::Debug,
    io::Seek,
    ops::{Deref, DerefMut},
    path::PathBuf,
};

use anyhow::anyhow;
use clap::Parser;
use dns_update::DnsUpdater;
use hcloud::{
    apis::{
        configuration::Configuration,
        networks_api::{self, ListNetworksParams},
        servers_api::{self, GetServerParams},
    },
    models::Network,
};
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the raw TSIG key.
    #[arg(long)]
    tsig_key_path: PathBuf,

    /// Name of the TSIG key.
    #[arg(long)]
    tsig_key_name: String,

    /// Address of the DNS server in the format "tcp|udp://ip:port".
    #[arg(long)]
    server_address: String,

    /// Hetzner HCloud API token.
    #[arg(long, env = "HCLOUD_API_TOKEN")]
    hcloud_api_token: String,

    /// Name of the private network in the Hetzner account.
    #[arg(long)]
    private_network_name: String,

    /// Directory to keep state in.
    #[arg(long, env = "STATE_DIRECTORY")]
    state_directory: PathBuf,

    /// DNS zone name.
    #[arg(long)]
    zone_name: String,

    /// If the private network name changes between invocations, this software will remove all DNS entries it previously created to clean up its state, and then start with a new state for the new network name. This flag indicates an acknowledgement of this behaviour. If not passed (or false), the software will exit with an error instead of cleaning things up.
    #[arg(long)]
    allow_private_network_change: bool,
}

#[derive(Debug)]
struct StateWrapper {
    file: std::fs::File,
    data: State,
}

impl StateWrapper {
    fn from_directory(dir: PathBuf) -> anyhow::Result<Self> {
        let state_path = dir.join("state.json");
        let state_file = std::fs::File::options()
            .create(true)
            .write(true)
            .read(true)
            .open(state_path)?;

        let state_data = if state_file.metadata()?.len() == 0 {
            State::default()
        } else {
            serde_json::from_reader(&state_file)?
        };

        Ok(Self {
            file: state_file,
            data: state_data,
        })
    }

    fn save(&mut self) -> anyhow::Result<()> {
        self.file.seek(std::io::SeekFrom::Start(0))?;
        serde_json::to_writer(&self.file, &self.data)?;

        Ok(())
    }
}

impl Drop for StateWrapper {
    fn drop(&mut self) {
        self.save().unwrap();
    }
}

impl Deref for StateWrapper {
    type Target = State;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl DerefMut for StateWrapper {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct State {
    private_network_name: String,
    servers_synced: Vec<Server>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
struct Server {
    id: i64,
    ip_address: String,
    hostname: String,
}

struct DnsUpdaterWrapper {
    client: DnsUpdater,
    zone_name: String,
}

// `DnsUpdater` doesn't impl Debug, so we need this.
impl Debug for DnsUpdaterWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DnsUpdaterWrapper {{}}")
    }
}

impl DnsUpdaterWrapper {
    #[tracing::instrument]
    fn new(
        server_address: String,
        key_name: String,
        key_path: PathBuf,
        zone_name: String,
    ) -> anyhow::Result<Self> {
        let tsig_key = std::fs::read(key_path)?;

        let client = DnsUpdater::new_rfc2136_tsig(
            server_address,
            key_name,
            tsig_key,
            dns_update::TsigAlgorithm::HmacSha256,
        )
        .map_err(|e| anyhow!("unable to create a DNS updater client. {}", e))?;

        Ok(Self { client, zone_name })
    }

    #[tracing::instrument]
    async fn add_server(&self, server: &Server) -> anyhow::Result<()> {
        tracing::debug!("Creating a DNS record for a server.");

        let server_fqdn = format!("{}.{}", server.hostname, self.zone_name);
        let server_ip_parsed = server.ip_address.parse()?;

        match self
            .client
            .create(
                &server_fqdn,
                dns_update::DnsRecord::A {
                    content: server_ip_parsed,
                },
                600,
                &self.zone_name,
            )
            .await
        {
            Ok(v) => Ok(v),
            Err(dns_update::Error::Response(resp_text)) => {
                tracing::warn!(resp_text, "Received a response error when trying to create a DNS record. We'll assume we got that because the record already exists, and will update it instead.");

                self.client
                    .update(
                        &server_fqdn,
                        dns_update::DnsRecord::A {
                            content: server_ip_parsed,
                        },
                        600,
                        &self.zone_name,
                    )
                    .await
                    .map_err(|e| anyhow!("failed to update a DNS record. {}", e))
            }
            Err(e) => Err(anyhow!("failed to create a DNS record. {}", e)),
        }?;

        Ok(())
    }

    #[tracing::instrument]
    async fn remove_server(&self, server: &Server) -> anyhow::Result<()> {
        tracing::debug!("Deleting a DNS record for a server.");

        self.client
            .delete(
                format!("{}.{}", server.hostname, self.zone_name),
                &self.zone_name,
            )
            .await
            .map_err(|e| anyhow!("failed to delete a DNS record. {}", e))?;

        Ok(())
    }
}

#[derive(Debug)]
struct HCloudWrapper {
    configuration: Configuration,
    network_name: String,

    // Quick cache to avoid getting the network multiple times.
    network_info: Option<Network>,
}

impl HCloudWrapper {
    fn new(api_token: String, network_name: String) -> Self {
        let mut configuration = Configuration::new();
        configuration.bearer_access_token = Some(api_token);

        Self {
            configuration,
            network_name,

            network_info: None,
        }
    }

    #[tracing::instrument(skip_all)]
    async fn retrieve_network(&mut self) -> anyhow::Result<()> {
        if self.network_info.is_some() {
            return Ok(());
        }

        tracing::debug!("Networking info wasn't retrieved yet. Will do that now.");

        let networks = networks_api::list_networks(
            &self.configuration,
            ListNetworksParams {
                name: Some(self.network_name.clone()),
                ..Default::default()
            },
        )
        .await?;

        if networks.networks.is_empty() {
            return Err(anyhow!(
                "Private network with name '{}' not found on the Hetzner account!",
                self.network_name
            ));
        }

        if networks.networks.len() > 1 {
            tracing::warn!("More than one network retrieved from the Hetzner API! Will proceed with the first one.");
        }

        self.network_info = Some(networks.networks.first().unwrap().clone());
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn server_ids(&mut self) -> anyhow::Result<Vec<i64>> {
        self.retrieve_network().await?;
        Ok(self.network_info.as_ref().unwrap().servers.clone())
    }

    #[tracing::instrument(skip_all)]
    async fn hydrate_server_list(&mut self, server_ids: Vec<i64>) -> anyhow::Result<Vec<Server>> {
        self.retrieve_network().await?;

        let network_id = self.network_info.as_ref().unwrap().id;
        let mut hydrated_servers = Vec::with_capacity(server_ids.len());

        for server_id in server_ids {
            let server_info =
                servers_api::get_server(&self.configuration, GetServerParams { id: server_id })
                    .await?;

            if let Some(server_info) = server_info.server {
                let current_server = Server {
                    id: server_id,
                    ip_address: server_info.private_net.iter().find(|n| n.network.is_some_and(|nid| nid == network_id)).and_then(|n| n.ip.clone()).ok_or_else(|| anyhow!("Server with id {} doesn't have a network with id {} attached to it!", server_id, network_id))?,
                    hostname: server_info.name,
                };

                hydrated_servers.push(current_server);
            } else {
                return Err(anyhow!(
                    "Couldn't get information for server with id {}!",
                    server_id
                ));
            }
        }

        Ok(hydrated_servers)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    tracing::info!("hetzner-private-dns-sync has initialising logging.");

    let args = Args::parse();

    let dns_updater = DnsUpdaterWrapper::new(
        args.server_address,
        args.tsig_key_name,
        args.tsig_key_path,
        args.zone_name,
    )?;
    tracing::info!("DNS Updater initialised.");
    let mut hcloud = HCloudWrapper::new(args.hcloud_api_token, args.private_network_name.clone());
    let mut current_state = StateWrapper::from_directory(args.state_directory)?;
    tracing::info!("Current state retrieved.");

    if current_state.private_network_name != args.private_network_name {
        if !current_state.servers_synced.is_empty() {
            if !args.allow_private_network_change {
                return Err(anyhow!("The private network name has changed, but the --allow-private-network-change flag was false! We'll exit with an error instead. If you expect the private network name to change and acknolwedge the behaviour of this software when that happens, pass the --allow-private-network-change flag to continue."));
            }

            tracing::warn!("The private network name has changed and we got the flag acknowledging we'll clean up the state. Will do that now.");
            for server_info in current_state.servers_synced.clone() {
                tracing::debug!(?server_info, "Removing record for server.");
                dns_updater.remove_server(&server_info).await?;
                current_state
                    .servers_synced
                    .retain(|s| s.id != server_info.id);
                current_state.save()?;
            }

            // We removed all the previous servers, so we can switch the private network name now.
            current_state.private_network_name = args.private_network_name;
            current_state.save()?;
        } else {
            // We're in a new state, so we'll populate the network name.
            current_state.private_network_name = args.private_network_name;
            current_state.save()?;
        }
    }

    let server_ids_from_state: HashSet<i64> =
        current_state.servers_synced.iter().map(|s| s.id).collect();
    let current_servers: HashSet<i64> = hcloud.server_ids().await?.into_iter().collect();
    let servers_to_add: Vec<i64> = current_servers
        .difference(&server_ids_from_state)
        .cloned()
        .collect();
    let servers_to_remove: Vec<i64> = server_ids_from_state
        .difference(&current_servers)
        .cloned()
        .collect();

    tracing::info!(
        ?servers_to_add,
        ?servers_to_remove,
        "Finished determining which servers got added and removed, will start updating things."
    );

    let servers_to_add = hcloud.hydrate_server_list(servers_to_add).await?;

    if !servers_to_add.is_empty() {
        for server_info in servers_to_add {
            tracing::debug!(?server_info, "Adding record for server.");
            dns_updater.add_server(&server_info).await?;
            current_state.servers_synced.push(server_info);
            current_state.save()?;
        }
    }

    if !servers_to_remove.is_empty() {
        for server_id in servers_to_remove {
            let server_info = current_state
                .servers_synced
                .iter()
                .find(|s| s.id == server_id)
                .unwrap();
            tracing::debug!(?server_info, "Removing record for server.");
            dns_updater.remove_server(&server_info).await?;
            let server_id = server_info.id;
            current_state.servers_synced.retain(|s| s.id != server_id);
            current_state.save()?;
        }
    }

    tracing::info!("Done!");
    Ok(())
}
