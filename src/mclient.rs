//
// https://www.github.com/8go/matrix-commander-rs
// mclient.rs
//

//! Module that bundles everything together that uses the `matrix-sdk` API.
//! Primarily the matrix_sdk::Client API
//! (see <https://docs.rs/matrix-sdk/latest/matrix_sdk/struct.Client.html>).
//! This module implements the matrix-sdk-based portions of the primitives like
//! logging in, logging out, verifying, sending messages, sending files, etc.

use std::borrow::Cow;
// use std::env;
use std::fs;
// use std::fs::File;
// use std::io::{self, Write};
// use std::ops::Deref;
// use std::path::Path;
use std::path::PathBuf;
use tracing::{debug, error, info, warn};
// use thiserror::Error;
// use directories::ProjectDirs;
// use serde::{Deserialize, Serialize};
use mime::Mime;
use url::Url;

use matrix_sdk::{
    attachment::AttachmentConfig,
    config::{RequestConfig, StoreConfig, SyncSettings},
    instant::Duration,
    // room,
    // room::Room,
    ruma::{
        events::room::message::{
            EmoteMessageEventContent, MessageType, NoticeMessageEventContent,
            RoomMessageEventContent, TextMessageEventContent,
        },
        RoomId,
        // OwnedRoomId, OwnedRoomOrAliasId, OwnedServerName,
        // device_id, room_id, session_id, user_id, OwnedDeviceId, OwnedUserId,
    },
    Client,
    // Session,
};

use crate::{credentials_exist, get_timeout, Credentials, GlobalState};
use crate::{Error, Sync}; // from main.rs

#[path = "emoji_verify.rs"]
mod emoji_verify; // import verification code

/// Constructor for matrix-sdk async Client, based on restore_login().
pub(crate) async fn restore_login(gs: &mut GlobalState) -> Result<Client, Error> {
    if gs.credentials_file_path.is_file() {
        let credentials = Credentials::load(&gs.credentials_file_path)?;
        let credentialsc1 = credentials.clone();
        let credentialsc2 = credentials.clone();
        gs.credentials = Some(credentials);
        let client = create_client(credentialsc1.homeserver, gs).await?;
        info!(
            "restoring device with device_id = {:?}",
            credentialsc1.device_id
        );
        client.restore_login(credentialsc2.into()).await?;
        // we skip sync if requested to do so
        sync_once(&client, get_timeout(gs), gs.ap.sync).await?;
        Ok(client)
    } else {
        Err(Error::NotLoggedIn)
    }
}

/// Constructor for matrix-sdk async Client, based on login_username().
pub(crate) async fn login<'a>(
    gs: &'a mut GlobalState,
    homeserver: &Url,
    username: &str,
    password: &str,
    device: &str,
    room_default: &str,
) -> Result<Client, Error> {
    let client = create_client(homeserver.clone(), gs).await?;
    debug!("About to call login_username()");
    let response = client
        .login_username(&username, password)
        .initial_device_display_name(device)
        .send()
        .await;
    debug!("Called login_username()");

    match response {
        Ok(n) => debug!("login_username() successful with response {:?}.", n),
        Err(e) => {
            error!("Error: {}", e);
            return Err(Error::LoginFailed);
        }
    }
    let session = client
        .session()
        .expect("error: client not logged in correctly. No session.");
    info!("device id = {}", session.device_id);
    info!("credentials file = {:?}", gs.credentials_file_path);

    Credentials::new(
        homeserver.clone(),
        session.user_id.clone(),
        session.access_token.clone(),
        session.device_id.clone(),
        room_default.to_string(),
        session.refresh_token.clone(),
    )
    .save(&gs.credentials_file_path)?;
    info!(
        "new credentials file created = {:?}",
        gs.credentials_file_path
    );
    sync_once(&client, get_timeout(gs), gs.ap.sync).await?;
    Ok(client)
}

/// Prepares a client that can then be used for actual login.
/// Configures the matrix-sdk async Client.
async fn create_client(homeserver: Url, gs: &GlobalState) -> Result<Client, Error> {
    // The location to save files to
    let sledhome = &gs.sledstore_dir_path;
    info!("Using sled store {:?}", &sledhome);
    // let builder = if let Some(proxy) = cli.proxy { builder.proxy(proxy) } else { builder };
    let builder = Client::builder()
        .homeserver_url(homeserver)
        .store_config(StoreConfig::new())
        .request_config(
            RequestConfig::new()
                .timeout(Duration::from_secs(get_timeout(gs)))
                .retry_timeout(Duration::from_secs(get_timeout(gs))),
        );
    let client = builder
        .sled_store(&sledhome, None)
        .expect("error: cannot add sled store to ClientBuilder.")
        .build()
        .await
        .expect("error: ClientBuilder build failed."); // no password for sled!
    Ok(client)
}

/// Does emoji verification
pub(crate) async fn verify(client: &Result<Client, Error>) -> Result<(), Error> {
    if let Ok(client) = client {
        // is logged in
        info!("Client logged in: {}", client.logged_in());
        info!("Client access token used: {:?}", client.access_token());
        emoji_verify::sync(&client).await?; // wait in sync for other party to initiate emoji verify
        Ok(())
    } else {
        Err(Error::NotLoggedIn)
    }
}

/// Logs out, destroying the device and removing credentials file
pub(crate) async fn logout(client: &Result<Client, Error>, gs: &GlobalState) -> Result<(), Error> {
    debug!("Logout on client");
    if let Ok(client) = client {
        // is logged in
        logout_server(&client).await?;
    }
    if credentials_exist(&gs) {
        match fs::remove_file(&gs.credentials_file_path) {
            Ok(()) => info!(
                "Credentials file successfully remove {:?}",
                &gs.credentials_file_path
            ),
            Err(e) => error!(
                "Error: credentials file not removed. {:?} {:?}",
                &gs.credentials_file_path, e
            ),
        }
    } else {
        warn!(
            "Credentials file does not exist {:?}",
            &gs.credentials_file_path
        )
    }

    match fs::remove_dir_all(&gs.sledstore_dir_path) {
        Ok(()) => info!(
            "Sled directory successfully remove {:?}",
            &gs.sledstore_dir_path
        ),
        Err(e) => error!(
            "Error: Sled directory not removed. {:?} {:?}",
            &gs.sledstore_dir_path, e
        ),
    }
    Ok(())
}

/// Only logs out from server, no local changes.
pub(crate) async fn logout_server(client: &Client) -> Result<(), Error> {
    match client.logout().await {
        Ok(n) => info!("Logout sent to server {:?}", n),
        Err(e) => error!(
            "Error: Server logout failed but we remove local device id anyway. {:?}",
            e
        ),
    }
    Ok(())
}

/// Utility function to synchronize once.
pub(crate) async fn sync_once(client: &Client, timeout: u64, stype: Sync) -> Result<(), Error> {
    debug!("value of sync in sync_once() is {:?}", stype);
    match stype {
        Sync::Off => {
            info!("syncing is turned off. No syncing.");
            Ok(())
        }
        Sync::Full => {
            info!("syncing once, timeout set to {} seconds ...", timeout);
            client
                .sync_once(SyncSettings::new().timeout(Duration::new(timeout, 0)))
                .await?; // sec
            info!("sync completed");
            Ok(())
        }
    }
}

/*pub(crate) fn room(&self, room_id: &RoomId) -> Result<room::Room> {
    self.get_room(room_id).ok_or(Error::InvalidRoom)
}*/

/*pub(crate) fn invited_room(&self, room_id: &RoomId) -> Result<room::Invited> {
    self.get_invited_room(room_id).ok_or(Error::InvalidRoom)
}*/

// pub(crate) fn joined_room(client: Client, room_id: &RoomId) -> Result<room::Joined> {
//     client.get_joined_room(room_id).ok_or(Error::InvalidRoom)
// }

/*pub(crate) fn left_room(&self, room_id: &RoomId) -> Result<room::Left> {
    self.get_left_room(room_id).ok_or(Error::InvalidRoom)
}*/

/// Get list of devices for the current user.
pub(crate) async fn devices(client: &Result<Client, Error>) -> Result<(), Error> {
    debug!("Devices on client");
    if let Ok(client) = client {
        // is logged in
        let response = client.devices().await?;
        for device in response.devices {
            println!(
                "Device: {} {}",
                device.device_id,
                device.display_name.as_deref().unwrap_or("")
            );
        }
        Ok(())
    } else {
        Err(Error::NotLoggedIn)
    }
}

/// Sent text message is various formats and types.
pub(crate) async fn message(
    client: &Result<Client, Error>,
    msg: String,
    room: String,
    code: bool,
    markdown: bool,
    notice: bool,
    emote: bool,
) -> Result<(), Error> {
    if client.is_err() {
        return Err(Error::InvalidClientConnection);
    }
    debug!("In message(): room is {}, msg is {}", room, msg);
    let (nmsg, md) = if code {
        let mut fmt_msg = String::from("```");
        // fmt_msg.push_str("name-of-language");  // Todo
        fmt_msg.push('\n');
        fmt_msg.push_str(&msg);
        if !fmt_msg.ends_with('\n') {
            fmt_msg.push('\n');
        }
        fmt_msg.push_str("```");
        (fmt_msg, true)
    } else {
        (msg, markdown)
    };

    let content = if notice {
        MessageType::Notice(if md {
            NoticeMessageEventContent::markdown(nmsg)
        } else {
            NoticeMessageEventContent::plain(nmsg)
        })
    } else if emote {
        MessageType::Emote(if md {
            EmoteMessageEventContent::markdown(nmsg)
        } else {
            EmoteMessageEventContent::plain(nmsg)
        })
    } else {
        MessageType::Text(if md {
            TextMessageEventContent::markdown(nmsg)
        } else {
            TextMessageEventContent::plain(nmsg)
        })
    };
    let proom = RoomId::parse(room).unwrap();
    debug!("In message(): parsed room is {:?}", proom);
    client
        .as_ref()
        .unwrap()
        .get_joined_room(&proom)
        .ok_or(Error::InvalidRoom)?
        .send(RoomMessageEventContent::new(content), None)
        .await?;
    Ok(())
}

/// Send a file of various Mime formats.
pub(crate) async fn file(
    client: &Result<Client, Error>,
    filename: PathBuf,
    room: String,          // RoomId
    label: Option<String>, // used as filename for attachment
    mime: Option<Mime>,
) -> Result<(), Error> {
    if client.is_err() {
        return Err(Error::InvalidClientConnection);
    }
    let data = fs::read(&filename)?;
    let proom = RoomId::parse(room).unwrap();
    client
        .as_ref()
        .unwrap()
        .get_joined_room(&proom)
        .ok_or(Error::InvalidRoom)?
        .send_attachment(
            label
                .as_ref()
                .map(Cow::from)
                .or_else(|| filename.file_name().as_ref().map(|o| o.to_string_lossy()))
                .ok_or(Error::InvalidFile)?
                .as_ref(),
            mime.as_ref().unwrap_or(
                &mime_guess::from_path(&filename).first_or(mime::APPLICATION_OCTET_STREAM),
            ),
            &data,
            AttachmentConfig::new(),
        )
        .await?;
    Ok(())
}