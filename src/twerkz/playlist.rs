//! Adds and removes local songs in Spotify playlists. The Web API refuses
//! `spotify:local:` items, so this writes the change the way Spotify's own
//! apps do, over the librespot session.

use anyhow::{Context, Result};
use http::Method;
use librespot_core::Session;
use librespot_core::spotify_id::SpotifyId;
use librespot_protocol::playlist4_external::{
    Add, ChangeInfo, Delta, Item, ItemAttributes, ListChanges, Op, SelectedListContent, op,
};
use protobuf::Message as _;

/// Appends `uris` to the end of the playlist.
pub async fn append(session: &Session, playlist_id: &str, uris: &[String]) -> Result<()> {
    let now = jiff::Timestamp::now().as_millisecond();
    let mut add = Add::new();
    add.set_add_last(true);
    for uri in uris {
        let mut item = Item::new();
        item.set_uri(uri.clone());
        let mut attributes = ItemAttributes::new();
        attributes.set_timestamp(now);
        attributes.set_added_by(session.username());
        item.attributes = Some(attributes).into();
        add.items.push(item);
    }
    let mut operation = Op::new();
    operation.set_kind(op::Kind::ADD);
    operation.add = Some(add).into();
    change(session, playlist_id, operation).await
}

async fn change(session: &Session, playlist_id: &str, operation: Op) -> Result<()> {
    let id = SpotifyId::from_base62(playlist_id).context("bad playlist id")?;
    let header = session
        .spclient()
        .get_playlist_range(&id, 0, 0)
        .await
        .context("cannot read the playlist")?;
    let current = SelectedListContent::parse_from_bytes(&header).context("unexpected playlist answer")?;
    let now = jiff::Timestamp::now().as_millisecond();

    let mut info = ChangeInfo::new();
    info.set_user(session.username());
    info.set_timestamp(now);

    let mut delta = Delta::new();
    delta.set_base_version(current.revision().to_vec());
    delta.ops.push(operation);
    delta.info = Some(info).into();

    let mut changes = ListChanges::new();
    changes.set_base_revision(current.revision().to_vec());
    changes.deltas.push(delta);
    changes.set_want_resulting_revisions(true);

    session
        .spclient()
        .request_with_protobuf(
            &Method::POST,
            &format!("/playlist/v2/playlist/{playlist_id}/changes"),
            None,
            &changes,
        )
        .await
        .context("Spotify refused the playlist change")?;
    Ok(())
}
