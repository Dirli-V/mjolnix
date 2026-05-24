use std::env;

use anyhow::{Context, Result, bail};
use mjolnix_shared::db::{self, DbPool};

pub async fn current_user_id(pool: &DbPool) -> Result<i64> {
    if let Ok(id) = env::var("MJOLNIX_USER_ID") {
        return id
            .parse()
            .with_context(|| format!("invalid MJOLNIX_USER_ID: {id}"));
    }
    if let Ok(fingerprint) = env::var("MJOLNIX_KEY_FINGERPRINT") {
        if let Some(user_id) = db::user_id_for_fingerprint(pool, &fingerprint).await? {
            return Ok(user_id);
        }
        let user_id = db::create_user(pool).await?;
        db::attach_key(pool, &fingerprint, user_id).await?;
        return Ok(user_id);
    }
    if env::var("SSH_CONNECTION").is_err() {
        return db::get_or_create_dev_user(pool).await;
    }
    bail!(
        "could not identify user: set MJOLNIX_USER_ID or MJOLNIX_KEY_FINGERPRINT in authorized_keys"
    )
}
