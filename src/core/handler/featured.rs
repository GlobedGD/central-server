use crate::features::{FeaturesError, FeaturesModule, PartialFeaturedLevelId};

use super::{ConnectionHandler, util::*};

struct ListResponse {
    levels: Vec<PartialFeaturedLevelId>,
    total_pages: u32,
}

impl ConnectionHandler {
    pub fn handle_get_featured_level(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        must_auth(client)?;

        let module = self.module::<FeaturesModule>();
        let level = module.get_featured_level_meta();

        let buf = data::encode_message!(self, 56, msg => {
            let mut msg = msg.init_featured_level();
            msg.set_level_id(level.id);
            msg.set_rate_tier(level.rate_tier);
            msg.set_edition(level.edition);
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    pub async fn handle_get_featured_list(
        &self,
        client: &ClientStateHandle,
        page: u32,
    ) -> HandlerResult<()> {
        must_auth(client)?;

        let module = self.module::<FeaturesModule>();

        let resp: Result<ListResponse, FeaturesError> = try {
            let levels = module.get_featured_levels_page(page).await?;
            let total_pages = module.get_featured_levels_total_pages().await?;

            ListResponse { levels, total_pages }
        };

        let resp = match resp {
            Ok(resp) => resp,
            Err(e) => {
                warn!("Failed to fetch featured levels (page {page}): {e}");
                self.send_warn(client, format!("Failed to fetch featured levels: {e}"))?;
                return Ok(());
            }
        };

        let cap = 80 + resp.levels.len() * 12;
        let buf = data::encode_message_heap!(self, cap, msg => {
            let mut msg = msg.init_featured_list();
            msg.set_page(page);
            msg.set_total_pages(resp.total_pages);
            let mut level_ids = msg.reborrow().init_level_ids(resp.levels.len() as u32);
            for (n, level) in resp.levels.iter().enumerate() {
                level_ids.set(n as u32, level.level_id);
            }

            let mut rate_tiers = msg.reborrow().init_rate_tiers(resp.levels.len() as u32);
            for (n, level) in resp.levels.iter().enumerate() {
                rate_tiers.set(n as u32, level.rate_tier as u8);
            }
        })?;

        client.send_data_bufkind(buf);

        Ok(())
    }

    pub async fn handle_send_featured_level(
        &self,
        client: &ClientStateHandle,
        level_id: i32,
        level_name: &str,
        author_id: i32,
        author_name: &str,
        rate_tier: u8,
        note: &str,
        queue: bool,
    ) -> HandlerResult<()> {
        self.must_be_able(
            client,
            if queue {
                super::ActionType::RateFeatures
            } else {
                super::ActionType::SendFeatures
            },
        )?;

        let module = self.module::<FeaturesModule>();

        let res = module
            .send_level(
                client.account_id(),
                level_id,
                level_name,
                author_id,
                author_name,
                rate_tier,
                note,
                queue,
            )
            .await;

        self.send_admin_db_result(client, res)?;

        Ok(())
    }
}
