use ashpd::zbus::{
    Connection, Proxy,
    fdo::PropertiesProxy,
    proxy::{Builder, CacheProperties},
};
use futures::{StreamExt as _, stream};
use gpui::PowerMode;

// The UPower namespace replaced net.hadess in v0.20; prefer it while retaining
// compatibility with older daemons.
// https://upower.pages.freedesktop.org/power-profiles-daemon/gdbus-org.freedesktop.UPower.PowerProfiles.html
// https://gitlab.freedesktop.org/upower/power-profiles-daemon/-/releases/0.20
const POWER_PROFILES_SERVICES: [(&str, &str); 2] = [
    (
        "org.freedesktop.UPower.PowerProfiles",
        "/org/freedesktop/UPower/PowerProfiles",
    ),
    ("net.hadess.PowerProfiles", "/net/hadess/PowerProfiles"),
];

fn power_mode(profile: &str) -> PowerMode {
    match profile {
        "power-saver" => PowerMode::LowPower,
        "balanced" => PowerMode::Balanced,
        "performance" => PowerMode::HighPower,
        _ => PowerMode::Unknown,
    }
}

pub(super) async fn monitor_power_mode(
    connection: &Connection,
    mut on_change: impl FnMut(PowerMode) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut proxies = Vec::new();
    let mut changes = Vec::new();
    for (service, path) in POWER_PROFILES_SERVICES {
        let proxy = Builder::<Proxy<'_>>::new(connection)
            .destination(service)?
            .path(path)?
            .interface(service)?
            .cache_properties(CacheProperties::No)
            .build()
            .await?;
        let properties = PropertiesProxy::builder(connection)
            .destination(service)?
            .path(path)?
            .build()
            .await?;
        changes.push(
            properties
                .receive_properties_changed()
                .await?
                .map(|_| ())
                .boxed(),
        );
        changes.push(proxy.receive_owner_changed().await?.map(|_| ()).boxed());
        proxies.push(proxy);
    }
    let mut changes = stream::select_all(changes);
    let mut previous_mode = None;
    loop {
        // Subscribe before reading, and bypass property caches so service loss
        // cannot leave us reporting the previous owner's profile.
        let mut mode = PowerMode::Unknown;
        for proxy in &proxies {
            match proxy.get_property::<String>("ActiveProfile").await {
                Ok(profile) => {
                    mode = power_mode(&profile);
                    break;
                }
                Err(error) => log::debug!(
                    "could not query power profile from {}: {error}",
                    proxy.destination()
                ),
            }
        }
        if previous_mode != Some(mode) {
            on_change(mode)?;
            previous_mode = Some(mode);
        }
        if changes.next().await.is_none() {
            on_change(PowerMode::Unknown)?;
            anyhow::bail!("power profile notification streams closed");
        }
    }
}
