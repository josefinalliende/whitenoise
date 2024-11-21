use crate::fetch_enriched_contact;
use crate::groups::{validate_group_members, Group, GroupType};
use crate::key_packages::fetch_key_packages_for_members;
use crate::secrets_store;
use crate::whitenoise::Whitenoise;
use nostr_openmls::groups::GroupError;
use nostr_sdk::prelude::*;
use std::collections::HashMap;
use std::ops::Add;
use tauri::Emitter;

/// Gets all MLS groups that the active account is a member of
/// This is scoped so that we can return only the groups that the user is a member of.
///
/// # Arguments
/// * `wn` - Whitenoise state containing account and group managers
///
/// # Returns
/// * `Ok(Vec<Group>)` - List of groups the active account belongs to
/// * `Err(String)` - Error message if retrieval fails
///
/// # Errors
/// Returns error if:
/// - No active account found
/// - Database error occurs retrieving groups
#[tauri::command]
pub fn get_groups(wn: tauri::State<'_, Whitenoise>) -> Result<Vec<Group>, String> {
    wn.group_manager.get_groups().map_err(|e| e.to_string())
}

/// Gets a single MLS group by its group ID
///
/// # Arguments
/// * `group_id` - Hex encoded MLS group ID
/// * `wn` - Whitenoise state
///
/// # Returns
/// * `Ok(Group)` - The requested group if found
/// * `Err(String)` - Error message if group not found or other error occurs
///
/// # Errors
/// Returns error if:
/// - Group ID is not valid hex
/// - Group not found in database
/// - Database error occurs
#[tauri::command]
pub fn get_group(group_id: String, wn: tauri::State<'_, Whitenoise>) -> Result<Group, String> {
    wn.group_manager
        .get_group(group_id)
        .map_err(|e| e.to_string())
}

/// Creates a new MLS group with the specified members and settings
///
/// # Arguments
/// * `creator_pubkey` - Public key of the group creator (must be the active account)
/// * `member_pubkeys` - List of public keys for group members
/// * `admin_pubkeys` - List of public keys for group admins
/// * `group_name` - Name of the group
/// * `description` - Description of the group
/// * `wn` - Whitenoise state
/// * `app_handle` - Tauri app handle
///
/// # Returns
/// * `Ok(Group)` - The newly created group
/// * `Err(String)` - Error message if group creation fails
///
/// # Flow
/// 1. Validates that active account is the creator and signer
/// 2. Validates member and admin lists
/// 3. Fetches key packages for all members
/// 4. Creates MLS group with NostrMls
/// 5. Sends welcome messages to all members via Nostr
/// 6. Adds group to GroupManager database
/// 7. Updates account with new group ID
/// 8. Emits group_added event
///
/// # Errors
/// Returns error if:
/// - Active account is not the creator
/// - Member/admin validation fails
/// - Key package fetching fails
/// - MLS group creation fails
/// - Welcome message sending fails
/// - Database operations fail
#[tauri::command]
pub async fn create_group(
    creator_pubkey: String,
    member_pubkeys: Vec<String>,
    admin_pubkeys: Vec<String>,
    group_name: String,
    description: String,
    wn: tauri::State<'_, Whitenoise>,
    app_handle: tauri::AppHandle,
) -> Result<Group, String> {
    let active_account = wn
        .account_manager
        .get_active_account()
        .map_err(|e| e.to_string())?;

    let signer = wn.nostr.client.signer().await.map_err(|e| e.to_string())?;

    // Check that active account is the creator and signer
    if active_account.pubkey != creator_pubkey
        || active_account.pubkey
            != signer
                .get_public_key()
                .await
                .map_err(|e| e.to_string())?
                .to_hex()
    {
        return Err("You must be the creator to create a group".to_string());
    }

    validate_group_members(&creator_pubkey, &member_pubkeys, &admin_pubkeys)
        .map_err(|e| e.to_string())?;

    let member_key_packages = fetch_key_packages_for_members(&member_pubkeys, &wn)
        .await
        .map_err(|e| e.to_string())?;

    tracing::debug!(
        target: "whitenoise::groups::create_group",
        "Member key packages: {:?}",
        member_key_packages
    );

    let mut group_relays: Vec<String>;
    {
        let nostr_settings = wn.nostr.settings.lock().unwrap();
        group_relays = nostr_settings.relays.clone();
        if tauri::is_dev() {
            group_relays.push("ws://localhost:8080".to_string());
        }
    }

    let create_group_result;
    {
        let nostr_mls = wn.nostr_mls.lock().expect("Failed to lock nostr_mls");

        create_group_result = nostr_mls
            .create_group(
                group_name,
                description,
                member_key_packages,
                admin_pubkeys,
                creator_pubkey,
                group_relays,
            )
            .map_err(|e| e.to_string())?;
    }

    let mls_group = create_group_result.mls_group;
    let serialized_welcome_message = create_group_result.serialized_welcome_message;
    let group_data = create_group_result.nostr_group_data;

    // Fan out the welcome message to all members
    for member in &member_pubkeys {
        let member_pubkey = PublicKey::from_hex(member).map_err(|e| e.to_string())?;
        let contact = fetch_enriched_contact(member.clone(), false, wn.clone(), app_handle.clone())
            .await
            .map_err(|e| e.to_string())?;

        let relay_urls = if !contact.inbox_relays.is_empty() {
            contact.inbox_relays
        } else if !contact.nostr_relays.is_empty() {
            contact.nostr_relays
        } else {
            // Get default relays from the client
            wn.nostr
                .client
                .relays()
                .await
                .keys()
                .map(|url| url.to_string())
                .collect()
        };

        let welcome_rumor = EventBuilder::new(
            Kind::MlsWelcome,
            hex::encode(&serialized_welcome_message),
            vec![Tag::from_standardized_without_cell(TagStandard::Relays(
                relay_urls
                    .iter()
                    .filter_map(|r| Url::parse(r).ok())
                    .collect(),
            ))],
        )
        .build(signer.get_public_key().await.map_err(|e| e.to_string())?);

        tracing::debug!(
            target: "whitenoise::groups::create_group",
            "Welcome rumor: {:?}",
            welcome_rumor
        );

        // Create a timestamp 1 month in the future
        let one_month_future = Timestamp::now().add(30 * 24 * 60 * 60);

        // TODO: We'll probably want to refactor this to be async eventually.
        let wrapped_event = EventBuilder::gift_wrap(
            &signer,
            &member_pubkey,
            welcome_rumor,
            vec![Tag::expiration(one_month_future)],
        )
        .await
        .map_err(|e| e.to_string())?;

        let max_retries = 5;
        let mut retry_count = 0;
        let mut last_error = None;

        while retry_count < max_retries {
            match wn
                .nostr
                .client
                .send_event_to(relay_urls.clone(), wrapped_event.clone())
                .await
            {
                Ok(_) => {
                    // Successfully sent, break the loop
                    break;
                }
                Err(e) => {
                    tracing::error!(
                        target: "whitenoise::groups::create_group",
                        "Failed to send welcome message to {:?}: {:?}",
                        &member_pubkey,
                        e
                    );
                    last_error = Some(e);
                    retry_count += 1;
                    if retry_count < max_retries {
                        // Wait for a short time before retrying
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
            }
        }

        if retry_count == max_retries {
            return Err(format!(
                "Failed to send event after {} attempts. Last error: {:?}",
                max_retries, last_error
            ));
        }

        tracing::debug!(
            target: "whitenoise::groups::create_group",
            "Published welcome message to {:?}",
            &member_pubkey
        );
    }

    let group_type = if mls_group.members().count() == 2 {
        GroupType::DirectMessage
    } else {
        GroupType::Group
    };

    let group_id = mls_group.group_id().to_vec();

    let nostr_group = wn
        .group_manager
        .add_group(
            group_id.clone(),
            mls_group.epoch().as_u64(),
            group_type,
            group_data,
        )
        .map_err(|e| e.to_string())?;

    tracing::debug!(
        target: "whitenoise::groups::create_group",
        "Added group to database: {:?}",
        nostr_group
    );

    app_handle
        .emit("group_added", nostr_group.clone())
        .map_err(|e| e.to_string())?;

    wn.account_manager
        .add_group_ids(
            active_account.pubkey,
            group_id.clone(),
            nostr_group.nostr_group_id.clone(),
        )
        .map_err(|e| e.to_string())?;

    tracing::debug!(
        target: "whitenoise::groups::create_group",
        "Added MLS group id to account manager: {:?}",
        group_id
    );

    Ok(nostr_group)
}

#[tauri::command]
pub async fn send_mls_message(
    group: Group,
    message: String,
    wn: tauri::State<'_, Whitenoise>,
    app_handle: tauri::AppHandle,
) -> Result<UnsignedEvent, String> {
    let nostr_keys = wn.nostr.client.signer().await.map_err(|e| e.to_string())?;

    // Create an unsigned nostr event with the message
    let mut inner_event = UnsignedEvent::new(
        nostr_keys
            .get_public_key()
            .await
            .map_err(|e| e.to_string())?,
        Timestamp::now(),
        Kind::Custom(9),
        Vec::new(),
        message,
    );
    inner_event.ensure_id();
    let json_event_string = serde_json::to_string(&inner_event).map_err(|e| e.to_string())?;

    let serialized_message;
    let export_secret_hex;
    let epoch;
    {
        let nostr_mls = wn.nostr_mls.lock().unwrap();
        serialized_message = nostr_mls
            .create_message_for_group(group.mls_group_id.clone(), json_event_string)
            .map_err(|e| e.to_string())?;

        (export_secret_hex, epoch) = nostr_mls
            .export_secret_as_hex_secret_key_and_epoch(group.mls_group_id.clone())
            .map_err(|e| e.to_string())?;
    }

    // Store the export secret key in the secrets store
    secrets_store::store_mls_export_secret(
        group.mls_group_id.clone(),
        epoch,
        export_secret_hex.clone(),
        wn.data_dir.as_path(),
    )
    .map_err(|e| e.to_string())?;

    let export_nostr_keys = Keys::parse(&export_secret_hex).map_err(|e| e.to_string())?;

    let encrypted_content = nip44::encrypt(
        export_nostr_keys.secret_key(),
        &export_nostr_keys.public_key(),
        &serialized_message,
        nip44::Version::V2,
    )
    .map_err(|e| e.to_string())?;

    let ephemeral_nostr_keys = Keys::generate();

    let published_message_event = EventBuilder::new(
        Kind::MlsGroupMessage,
        encrypted_content,
        vec![Tag::custom(
            TagKind::h(),
            vec![group.nostr_group_id.clone()],
        )],
    )
    .sign(&ephemeral_nostr_keys)
    .await
    .map_err(|e| e.to_string())?;

    tracing::debug!(
        target: "whitenoise::commands::groups::send_mls_message",
        "Publishing MLSMessage event to group relays"
    );

    let relays = if tauri::is_dev() {
        vec!["ws://localhost:8080".to_string()]
    } else {
        group.relay_urls.clone()
    };

    wn.nostr
        .client
        .send_event_to(relays, published_message_event)
        .await
        .map_err(|e| e.to_string())?;

    let new_group = wn
        .group_manager
        .add_message_to_group(group.mls_group_id.clone(), inner_event.clone())
        .map_err(|e| e.to_string())?;

    app_handle
        .emit("mls_message_sent", (new_group, inner_event.clone()))
        .expect("Couldn't emit event");

    Ok(inner_event)
}

// TODO: Make this use last synced so we don't fetch things we don't need repeatedly.
// TODO: Maybe split this into a method to handle groups individually?
#[tauri::command]
pub async fn fetch_mls_messages(
    wn: tauri::State<'_, Whitenoise>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let group_ids: Vec<String> = wn
        .group_manager
        .get_groups()
        .expect("Failed to get groups")
        .iter()
        .map(|group| group.nostr_group_id.clone())
        .collect();

    let message_events = wn
        .nostr
        .query_mls_group_messages(group_ids)
        .await
        .map_err(|e| e.to_string())?;

    let grouped_messages = message_events
        .into_iter()
        .filter_map(|event| {
            event
                .tags
                .iter()
                .find(|tag| tag.kind() == TagKind::h())
                .and_then(|tag| {
                    tag.content()
                        .map(|group_id| (group_id.to_string(), event.clone()))
                })
        })
        .fold(
            HashMap::new(),
            |mut acc: HashMap<String, Vec<Event>>, (group_id, event)| {
                acc.entry(group_id).or_default().push(event);
                acc
            },
        );

    tracing::debug!(
        target: "whitenoise::commands::groups::fetch_mls_messages",
        "Grouped messages: {:?}",
        grouped_messages
    );

    for (group_id, events) in grouped_messages {
        let group = wn
            .group_manager
            .get_group_by_nostr_id(group_id)
            .map_err(|e| e.to_string())?;

        // Sort the events by created_at (and then lexigraphically by ID)
        let mut sorted_events = events.into_iter().collect::<Vec<_>>();
        sorted_events.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));

        for event in sorted_events {
            // TODO: Should we track the id's of the message_events that we've already processed?

            tracing::debug!(
                target: "whitenoise::commands::groups::fetch_mls_messages",
                "Processing event: {:?}",
                event.id
            );

            let nostr_keys = match secrets_store::get_export_secret_keys_for_group(
                group.mls_group_id.clone(),
                group.epoch,
                wn.data_dir.as_path(),
            ) {
                Ok(keys) => keys,
                Err(_) => {
                    tracing::debug!(
                        target: "whitenoise::commands::groups::fetch_mls_messages",
                        "No export secret keys found, fetching from nostr_openmls",
                    );
                    // We need to get the export secret for the group from nostr_openmls
                    let nostr_mls = wn.nostr_mls.lock().unwrap();
                    let (export_secret_hex, epoch) = nostr_mls
                        .export_secret_as_hex_secret_key_and_epoch(group.mls_group_id.clone())
                        .map_err(|e| e.to_string())?;

                    // Store the export secret key in the secrets store
                    secrets_store::store_mls_export_secret(
                        group.mls_group_id.clone(),
                        epoch,
                        export_secret_hex.clone(),
                        wn.data_dir.as_path(),
                    )
                    .map_err(|e| e.to_string())?;

                    Keys::parse(&export_secret_hex).map_err(|e| e.to_string())?
                }
            };

            // Decrypt events using export secret key
            let decrypted_content = nip44::decrypt_to_bytes(
                nostr_keys.secret_key(),
                &nostr_keys.public_key(),
                &event.content,
            )
            .map_err(|e| format!("Error decrypting message: {}", e))?;

            let message_vec;
            {
                let nostr_mls = wn.nostr_mls.lock().unwrap();

                match nostr_mls.process_message_for_group(
                    group.mls_group_id.clone(),
                    decrypted_content.clone(),
                ) {
                    Ok(messages) => message_vec = messages,
                    Err(e) => {
                        match e {
                            GroupError::ProcessMessageError(_) => {
                                tracing::error!(
                                    target: "whitenoise::commands::groups::fetch_mls_messages",
                                    "Error processing message for group: {}",
                                    e
                                );
                            }
                            _ => {
                                tracing::error!(
                                    target: "whitenoise::commands::groups::fetch_mls_messages",
                                    "UNRECOGNIZED ERROR processing message for group: {}",
                                    e
                                );
                            }
                        }
                        continue;
                    }
                }
            }

            // This processes an application message into JSON.
            match serde_json::from_slice::<serde_json::Value>(&message_vec) {
                Ok(json_value) => {
                    tracing::debug!(
                        target: "whitenoise::commands::groups::fetch_mls_messages",
                        "Deserialized JSON message: {}",
                        json_value
                    );
                    let json_str = json_value.to_string();
                    let json_event = UnsignedEvent::from_json(&json_str).unwrap();
                    // Check to make sure we don't already have this event in the transcript
                    if !group.transcript.iter().any(|e| e.id == json_event.id) {
                        tracing::debug!(
                            target: "whitenoise::commands::groups::fetch_mls_messages",
                            "Adding new message to transcript: {:?}",
                            json_event.id
                        );

                        wn.group_manager
                            .add_message_to_group(group.mls_group_id.clone(), json_event.clone())
                            .map_err(|e| e.to_string())?;

                        app_handle
                            .emit("mls_message_received", (group.clone(), json_event.clone()))
                            .expect("Couldn't emit event");
                    }
                }
                Err(e) => {
                    tracing::error!(
                        target: "whitenoise::commands::groups::fetch_mls_messages",
                        "Failed to deserialize message into JSON: {}",
                        e
                    );
                }
            }
            // TODO: Handle Proposal
            // TODO: Handle Commit
            // TODO: Handle External Join
        }

        // emit events to let the front end know
    }

    Ok(())
}
