use crate::accounts::Account;
use crate::whitenoise::Whitenoise;
use nostr_sdk::prelude::*;

/// Retrieves the currently active account.
///
/// # Arguments
///
/// * `wn` - A reference to the Whitenoise state.
///
/// # Returns
///
/// * `Ok(Option<Account>)` - The active account if it exists.
/// * `Err(String)` - An error message if there was an issue fetching the active account.
#[tauri::command]
pub fn get_active_account(wn: tauri::State<'_, Whitenoise>) -> Result<Account, String> {
    Account::get_active(&wn).map_err(|e| e.to_string())
}

/// Lists all accounts.
///
/// # Arguments
///
/// * `wn` - A reference to the Whitenoise state.
///
/// # Returns
///
/// * `Ok(Vec<Account>)` - A vector of accounts if successful.
/// * `Err(String)` - An error message if there was an issue listing the accounts.
#[tauri::command]
pub fn get_accounts(wn: tauri::State<'_, Whitenoise>) -> Result<Vec<Account>, String> {
    Account::all(&wn).map_err(|e| format!("Error fetching accounts: {}", e))
}

/// Creates a new identity by generating a new keypair and logging in with it.
///
/// # Arguments
///
/// * `wn` - A reference to the Whitenoise state.
/// * `app_handle` - The app handle.
///
/// # Returns
///
/// * `Ok(Account)` - The newly created account.
/// * `Err(String)` - An error message if there was an issue creating the identity.
#[tauri::command]
pub async fn create_identity(
    wn: tauri::State<'_, Whitenoise>,
    app_handle: tauri::AppHandle,
) -> Result<Account, String> {
    let account = Account::new(&wn).map_err(|e| format!("Error creating account: {}", e))?;
    account
        .set_active(&wn, &app_handle)
        .await
        .map_err(|e| format!("Error setting active account: {}", e))
}

/// Logs in with the given public key. Will set the active account if successful.
///
/// # Arguments
///
/// * `wn` - A reference to the Whitenoise state.
/// * `hex_pubkey` - The public key in hexadecimal format.
///
/// # Returns
///
/// * `Ok(Account)` - The account if login was successful.
/// * `Err(String)` - An error message if there was an issue logging in.
#[tauri::command]
pub async fn login(
    nsec_or_hex_privkey: String,
    wn: tauri::State<'_, Whitenoise>,
    app_handle: tauri::AppHandle,
) -> Result<Account, String> {
    let keys = Keys::parse(&nsec_or_hex_privkey).map_err(|e| e.to_string())?;

    match Account::find_by_pubkey(&keys.public_key, &wn) {
        Ok(account) => {
            tracing::debug!("Account found, setting active");
            account
                .set_active(&wn, &app_handle)
                .await
                .map_err(|e| format!("Error logging in: {}", e))
        }
        _ => {
            tracing::debug!(target: "whitenoise::commands::accounts","Account not found, adding from keys");
            Account::add_from_keys(&keys, true, &wn, &app_handle)
                .await
                .map_err(|e| format!("Error logging in: {}", e))
        }
    }
}

/// Sets the active account.
///
/// # Arguments
///
/// * `wn` - A reference to the Whitenoise state.
/// * `hex_pubkey` - The public key in hexadecimal format.
///
/// # Returns
///
/// * `Ok(())` - If the active account was set successfully.
/// * `Err(String)` - An error message if there was an issue setting the active account.
#[tauri::command]
pub async fn set_active_account(
    hex_pubkey: String,
    wn: tauri::State<'_, Whitenoise>,
    app_handle: tauri::AppHandle,
) -> Result<Account, String> {
    let pubkey =
        PublicKey::parse(&hex_pubkey).map_err(|e| format!("Error parsing public key: {}", e))?;
    let account = Account::find_by_pubkey(&pubkey, &wn)
        .map_err(|e| format!("Error fetching account: {}", e))?;
    account
        .set_active(&wn, &app_handle)
        .await
        .map_err(|e| format!("Error setting active account: {}", e))
}

/// Logs out the specified account.
///
/// This function:
/// 1. Removes the account from the account manager
/// 2. Removes the private key from the secrets store
/// 3. Updates the Nostr identity to the new active account if needed
///
/// # Arguments
///
/// * `wn` - A reference to the Whitenoise state
/// * `hex_pubkey` - The public key in hexadecimal format of the account to log out
///
/// # Returns
///
/// * `Ok(())` - If the logout was successful
/// * `Err(String)` - An error message if there was an issue during logout
#[tauri::command]
pub async fn logout(
    hex_pubkey: String,
    wn: tauri::State<'_, Whitenoise>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let pubkey =
        PublicKey::parse(&hex_pubkey).map_err(|e| format!("Error parsing public key: {}", e))?;
    let account = Account::find_by_pubkey(&pubkey, &wn).expect("No account found");
    account
        .remove(&wn, app_handle)
        .await
        .map_err(|e| format!("Error logging out: {}", e))
}

/// Updates the onboarding status for a specific account.
///
/// # Arguments
///
/// * `pubkey` - The public key of the account to update
/// * `inbox_relays` - Whether inbox relays have been configured
/// * `key_package_relays` - Whether key package relays have been configured
/// * `publish_key_package` - Whether the key package has been published
/// * `wn` - A reference to the Whitenoise state
///
/// # Returns
///
/// * `Ok(Account)` - The updated account if successful
/// * `Err(String)` - An error message if there was an issue updating the account
#[tauri::command]
pub fn update_account_onboarding(
    pubkey: String,
    inbox_relays: bool,
    key_package_relays: bool,
    publish_key_package: bool,
    wn: tauri::State<'_, Whitenoise>,
) -> Result<Account, String> {
    let pubkey =
        PublicKey::parse(&pubkey).map_err(|e| format!("Error parsing public key: {}", e))?;
    let mut account = Account::find_by_pubkey(&pubkey, &wn)
        .map_err(|e| format!("Error fetching account: {}", e))?;
    account.onboarding.inbox_relays = inbox_relays;
    account.onboarding.key_package_relays = key_package_relays;
    account.onboarding.publish_key_package = publish_key_package;
    account
        .save(&wn)
        .map_err(|e| format!("Error saving account: {}", e))?;
    Ok(account)
}
