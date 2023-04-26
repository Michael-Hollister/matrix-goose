use std::ops::Deref;

use thiserror::Error;
use tracing::{instrument, warn};

use super::{Joined, Left};
// use crate::{
//     room::{Common, RoomMember},
//     BaseRoom, Client, Error, Result, RoomState,
// };
use matrix_sdk::{
    BaseRoom,
};
// use ruma::{
//     api::client::{
//         membership::{join_room_by_id, leave_room},
//     },
// };
use crate::matrix::{
    room::{Common, RoomMember, RoomState},
    error::{Error, Result},
    GooseMatrixClient
    // GooseMatrixClient, MatrixResponse, MatrixError,
};

/// A room in the invited state.
///
/// This struct contains all methods specific to a `Room` with
/// `RoomState::Invited`. Operations may fail once the underlying `Room` changes
/// `RoomState`.
#[derive(Debug, Clone)]
pub struct Invited {
    pub(crate) inner: Common,
}

/// Details of the (latest) invite.
#[derive(Debug, Clone)]
pub struct Invite {
    /// Who has been invited.
    pub invitee: RoomMember,
    /// Who sent the invite.
    pub inviter: Option<RoomMember>,
}

#[derive(Error, Debug)]
pub enum InvitationError {
    /// The client isn't logged in.
    #[error("The client isn't authenticated")]
    NotAuthenticated,
    #[error("No membership event found")]
    EventMissing,
}

impl Invited {
    /// Create a new `room::Invited` if the underlying `Room` has
    /// `RoomState::Invited`.
    ///
    /// # Arguments
    /// * `client` - The client used to make requests.
    ///
    /// * `room` - The underlying room.
    // pub(crate) fn new(client: &Client, room: BaseRoom) -> Option<Self> {
    pub(crate) fn new(client: &GooseMatrixClient, room: BaseRoom) -> Option<Self> {
        if room.state() == RoomState::Invited {
            Some(Self { inner: Common::new(client.clone(), room) })
        } else {
            None
        }
    }

    /// Reject the invitation.
    pub async fn reject_invitation(&self) -> Result<Left> {
    //pub async fn reject_invitation(&self) -> Result<MatrixResponse<leave_room::v3::Response>, MatrixError<Error>> {
        self.inner.leave().await
    }

    /// Accept the invitation.
    #[instrument(skip_all)]
    pub async fn accept_invitation(&self) -> Result<Joined> {
    //pub async fn accept_invitation(&self) -> Result<MatrixResponse<join_room_by_id::v3::Response>, MatrixError<Error>> {
        let joined = self.inner.join().await?;
        let is_direct_room = self.inner.is_direct().await.unwrap_or_else(|e| {
            warn!(room_id = ?self.room_id(), "is_direct() failed: {e}");
            false
        });

        if is_direct_room {
            _ = self.inner.set_is_direct(true).await;
        }

        Ok(joined)
    }

    /// The membership details of the (latest) invite for this room.
    pub async fn invite_details(&self) -> Result<Invite> {
        let user_id = self
            .inner
            .client
            .user_id()
            .ok_or_else(|| Error::UnknownError(Box::new(InvitationError::NotAuthenticated)))?;
        let invitee = self
            .inner
            .get_member_no_sync(user_id)
            .await?
            .ok_or_else(|| Error::UnknownError(Box::new(InvitationError::EventMissing)))?;
        let event = invitee.event();
        let inviter_id = event.sender();
        let inviter = self.inner.get_member_no_sync(inviter_id).await?;
        Ok(Invite { invitee, inviter })
    }
}

impl Deref for Invited {
    type Target = Common;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
