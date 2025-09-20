use thiserror::Error;
use generic_async_http_client::{Error as RequestError, Request};
use serde::Serialize;

#[derive(Clone, Debug)]
pub struct GDUser {
    pub account_id: i32,
    pub user_id: i32,
    pub username: heapless::String<24>,
    pub display_name: heapless::String<24>,
    pub cube: i16,
    pub color1: u16,
    pub color2: u16,
    pub glow_color: u16,
}

impl Default for GDUser {
    fn default() -> Self {
        Self {
            account_id: -1,
            user_id: -1,
            username: heapless::String::new(),
            display_name: heapless::String::new(),
            cube: 1,
            color1: 1,
            color2: 3,
            glow_color: u16::MAX,
        }
    }
}

#[derive(Debug, Error)]
pub enum GDApiFetchError {
    #[error("Error making request: {0}")]
    Network(#[from] RequestError),
    #[error("Rate limited by cloudflare (error 1015)")]
    RateLimited,
    #[error("IP banned by cloudflare (error 1006)")]
    IpBlocked,
    #[error("ISP blocked by cloudflare (error 1005)")]
    AsnBlocked,
    #[error("Unknown cloudflare error: {0}")]
    Cloudflare(i32),
    #[error("GD server returned error code {0}")]
    BoomlingsError(i32),
    #[error("GD server returned invalid response")]
    BoomlingsUnparsable,
    #[error("GD server returned invalid user data")]
    InvalidUser,
}

#[derive(Serialize)]
pub struct GetUserInfoPayload {
    secret: &'static str,
    #[serde(rename = "targetAccountID")]
    target: i32,
}

#[derive(Serialize)]
pub struct GetUsersPayload {
    secret: &'static str,
    #[serde(rename = "str")]
    target: String,
}

pub struct GDApiClient {}

impl Default for GDApiClient {
    fn default() -> Self {
        Self {}
    }
}

impl GDApiClient {
    // returns a GDUser from a server response string
    pub fn user_from_string(&self, text: String) -> Result<Option<GDUser>, GDApiFetchError> {
        let mut user = GDUser {
            ..Default::default()
        };

        let mut no_glow = false;

        text.split(':').array_chunks::<2>().for_each(|[k, v]| match k {
            "10" if let Ok(c) = v.parse() => user.color1 = c,
            "16" if let Ok(c) = v.parse() => user.account_id = c,
            "11" if let Ok(c) = v.parse() => user.color2 = c,
            "51" if let Ok(c) = v.parse()
                && !no_glow =>
            {
                user.glow_color = c
            }
            "28" if v == "0" => {
                no_glow = true;
                user.glow_color = u16::MAX;
            }
            "21" if let Ok(c) = v.parse() => user.cube = c,
            "2" if let Ok(c) = v.parse() => user.user_id = c,
            "1" => {
                user.username =
                    heapless::String::try_from(v).unwrap_or_else(|_| "Unknown".try_into().unwrap())
            }
            _ => {}
        });

        if user.account_id <= 0
            || user.user_id <= 0
            || user.username.is_empty()
            || !user.username.is_ascii()
        {
            return Err(GDApiFetchError::InvalidUser);
        }

        Ok(Some(user))
    }

    // fetches a GDUser from boomlings by account ID
    pub async fn fetch_user(&self, account_id: i32) -> Result<Option<GDUser>, GDApiFetchError> {
        // TODO: uh gdps
        let text = Request::post("http://www.boomlings.com/database/getGJUserInfo20.php")
            .add_header("User-Agent", "")?
            .form(&GetUserInfoPayload {
                secret: "Wmfd2893gb7",
                target: account_id,
            })?
            .exec()
            .await?
            .text()
            .await?;

        if let Some(text) = text.strip_prefix("error code:").map(|x| x.trim()) {
            match text.parse::<i32>() {
                Ok(1005) => return Err(GDApiFetchError::AsnBlocked),
                Ok(1006) => return Err(GDApiFetchError::IpBlocked),
                Ok(1015) => return Err(GDApiFetchError::RateLimited),
                Ok(code) => return Err(GDApiFetchError::Cloudflare(code)),
                Err(_) => return Err(GDApiFetchError::BoomlingsUnparsable),
            }
        }

        if let Ok(ec) = text.parse::<i32>() {
            match ec {
                -1 => return Ok(None),
                _ => return Err(GDApiFetchError::BoomlingsError(ec)),
            }
        }

        let Ok(Some(user)) = self.user_from_string(text) else {
            return Err(GDApiFetchError::InvalidUser);
        };

        Ok(Some(user))
    }

    // fetches a GDUser from boomlings by username
    pub async fn fetch_user_by_username(&self, username: &String) -> Result<Option<GDUser>, GDApiFetchError> {
        // TODO: uh gdps
        let text = Request::post("http://www.boomlings.com/database/getGJUsers20.php")
            .add_header("User-Agent", "")?
            .form(&GetUsersPayload {
                secret: "Wmfd2893gb7",
                target: username.to_string(),
            })?
            .exec()
            .await?
            .text()
            .await?;

        if let Some(text) = text.strip_prefix("error code:").map(|x| x.trim()) {
            match text.parse::<i32>() {
                Ok(1005) => return Err(GDApiFetchError::AsnBlocked),
                Ok(1006) => return Err(GDApiFetchError::IpBlocked),
                Ok(1015) => return Err(GDApiFetchError::RateLimited),
                Ok(code) => return Err(GDApiFetchError::Cloudflare(code)),
                Err(_) => return Err(GDApiFetchError::BoomlingsUnparsable),
            }
        }

        if let Ok(ec) = text.parse::<i32>() {
            match ec {
                -1 => return Ok(None),
                _ => return Err(GDApiFetchError::BoomlingsError(ec)),
            }
        }

        let Ok(Some(user)) = self.user_from_string(text) else {
            return Err(GDApiFetchError::InvalidUser);
        };

        Ok(Some(user))
    }
}
