use crate::features::FeaturesModule;

use super::{ConnectionHandler, util::*};

impl ConnectionHandler {
    pub fn handle_get_featured_level(&self, client: &ClientStateHandle) -> HandlerResult<()> {
        must_auth(client)?;

        let module = self.module::<FeaturesModule>();
        let level = module.get_featured_level_id();

        let buf = data::encode_message!(self, 48, msg => {
            msg.init_featured_level().set_level_id(level);
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
