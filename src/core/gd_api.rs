use std::sync::LazyLock;

use generic_async_http_client::{Error as RequestError, Request};
use parking_lot::Mutex;
use serde::Serialize;
use thiserror::Error;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum GDDifficulty {
    #[default]
    NA = -1,
    Auto = 0,
    Easy = 1,
    Normal = 2,
    Hard = 3,
    Harder = 4,
    Insane = 5,
    Demon = 6,
    DemonEasy = 7,
    DemonMedium = 8,
    DemonInsane = 9,
    DemonExtreme = 10,
}

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

#[derive(Clone, Debug)]
pub struct GDLevel {
    pub id: i32,
    pub name: heapless::String<32>,
    pub author_id: i32,
    pub author_name: heapless::String<24>,
    pub difficulty: GDDifficulty,
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

impl Default for GDLevel {
    fn default() -> Self {
        Self {
            id: -1,
            name: heapless::String::new(),
            author_id: -1,
            author_name: heapless::String::new(),
            difficulty: GDDifficulty::NA,
        }
    }
}

impl GDDifficulty {
    pub fn new(val: i32) -> Self {
        match val {
            0 => GDDifficulty::Auto,
            1 => GDDifficulty::Easy,
            2 => GDDifficulty::Normal,
            3 => GDDifficulty::Hard,
            4 => GDDifficulty::Harder,
            5 => GDDifficulty::Insane,
            6 => GDDifficulty::Demon,
            7 => GDDifficulty::DemonEasy,
            8 => GDDifficulty::DemonMedium,
            9 => GDDifficulty::DemonInsane,
            10 => GDDifficulty::DemonExtreme,
            _ => GDDifficulty::NA,
        }
    }

    pub fn is_demon(&self) -> bool {
        matches!(
            self,
            GDDifficulty::Demon
                | GDDifficulty::DemonEasy
                | GDDifficulty::DemonMedium
                | GDDifficulty::DemonInsane
                | GDDifficulty::DemonExtreme
        )
    }
}

#[derive(Debug, Error)]
pub enum GDApiFetchError {
    #[error("Error making request: {0}")]
    Network(Box<RequestError>),
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

impl From<RequestError> for GDApiFetchError {
    fn from(e: RequestError) -> Self {
        GDApiFetchError::Network(Box::new(e))
    }
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

#[derive(Serialize)]
pub struct GetLevelsPayload {
    secret: &'static str,
    #[serde(rename = "str")]
    target: String,
    r#type: i32,
    #[serde(rename = "gameVersion")]
    game_version: i32,
    #[serde(rename = "binaryVersion")]
    binary_version: i32,
}

// global var for url and auth token
static BASE_URL: LazyLock<Mutex<String>> =
    LazyLock::new(|| Mutex::new(String::from("https://www.boomlings.com/database")));
static AUTH_TOKEN: LazyLock<Mutex<Option<String>>> = LazyLock::new(|| Mutex::new(None));

#[derive(Default)]
pub struct GDApiClient {
    base_url: Option<String>,
}

impl GDApiClient {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_base_url(url: String) -> Self {
        let mut ret = Self::default();
        ret.set_base_url(url);
        ret
    }

    pub fn set_base_url(&mut self, mut url: String) {
        while url.ends_with('/') {
            url.pop();
        }

        self.base_url = Some(url);
    }

    pub fn set_global_base_url(mut url: String) {
        while url.ends_with('/') {
            url.pop();
        }

        let mut guard = BASE_URL.lock();
        *guard = url;
    }

    pub fn set_global_auth_token(token: String) {
        let mut guard = AUTH_TOKEN.lock();
        *guard = Some(token);
    }

    fn make_url(&self, suffix: &str) -> String {
        match self.base_url.as_deref() {
            Some(base) => format!("{}/{}", base, suffix),
            None => {
                let base = &**BASE_URL.lock();
                format!("{}/{}", base, suffix)
            }
        }
    }

    async fn send_request(
        &self,
        url: &str,
        payload: &impl Serialize,
    ) -> Result<String, GDApiFetchError> {
        let mut req = Request::post(url).add_header("User-Agent", "")?;

        if let Some(token) = AUTH_TOKEN.lock().as_deref() {
            req = req.add_header("Authorization", token)?;
        }

        let text = req.form(payload)?.exec().await?.text().await?;

        if let Some(text) = text.strip_prefix("error code:").map(|x| x.trim()) {
            match text.parse::<i32>() {
                Ok(1005) => return Err(GDApiFetchError::AsnBlocked),
                Ok(1006) => return Err(GDApiFetchError::IpBlocked),
                Ok(1015) => return Err(GDApiFetchError::RateLimited),
                Ok(code) => return Err(GDApiFetchError::Cloudflare(code)),
                Err(_) => return Err(GDApiFetchError::BoomlingsUnparsable),
            }
        }

        Ok(text)
    }

    // fetches a GDUser from boomlings by account ID
    pub async fn fetch_user(&self, account_id: i32) -> Result<Option<GDUser>, GDApiFetchError> {
        let text = self
            .send_request(
                &self.make_url("getGJUserInfo20.php"),
                &GetUserInfoPayload {
                    secret: "Wmfd2893gb7",
                    target: account_id,
                },
            )
            .await?;

        if let Ok(ec) = text.parse::<i32>() {
            match ec {
                -1 => return Ok(None),
                _ => return Err(GDApiFetchError::BoomlingsError(ec)),
            }
        }

        self.user_from_string(&text)
    }

    // fetches a GDUser from boomlings by username
    pub async fn fetch_user_by_username(
        &self,
        username: &String,
    ) -> Result<Option<GDUser>, GDApiFetchError> {
        let text = self
            .send_request(
                &self.make_url("getGJUsers20.php"),
                &GetUsersPayload {
                    secret: "Wmfd2893gb7",
                    target: username.to_string(),
                },
            )
            .await?;

        if let Ok(ec) = text.parse::<i32>() {
            match ec {
                -1 => return Ok(None),
                _ => return Err(GDApiFetchError::BoomlingsError(ec)),
            }
        }

        self.user_from_string(&text)
    }

    // fetches a GDLevel from boomlings by level ID
    pub async fn fetch_level(&self, level_id: i32) -> Result<Option<GDLevel>, GDApiFetchError> {
        let text = self
            .send_request(
                &self.make_url("getGJLevels21.php"),
                &GetLevelsPayload {
                    secret: "Wmfd2893gb7",
                    target: level_id.to_string(),
                    r#type: 0,
                    game_version: 22,
                    binary_version: 45,
                },
            )
            .await?;

        if let Ok(ec) = text.parse::<i32>() {
            match ec {
                -1 => return Ok(None),
                _ => return Err(GDApiFetchError::BoomlingsError(ec)),
            }
        }

        self.level_from_string(&text)
    }

    // returns a GDUser from a server response string
    fn user_from_string(&self, text: &str) -> Result<Option<GDUser>, GDApiFetchError> {
        let mut user = GDUser::default();

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

    fn level_from_string(&self, text: &str) -> Result<Option<GDLevel>, GDApiFetchError> {
        // Example response:
        // 1:123123123:2:123:5:8:6:50164049:8:10:9:30:10:4781:12:0:13:22:14:133:17::43:0:25::18:0:19:0:42:0:45:1401:3:VmVyeSBrb29sIGlkIDpEIC0gVXBkYXRlIHNvb24_:15:1:30:0:31:0:37:0:38:0:39:5:46:1:47:2:35:10003129#50164049:Lasokar:11982945#1~|~10003129~|~2~|~Scheming Weasel faster~|~3~|~10001856~|~4~|~Kevin MacLeod~|~5~|~1.3~|~6~|~~|~10~|~-~|~7~|~UCSZXFhRIx6b0dFX3xS8L1yQ~|~8~|~1#9999:0:10#e59e08b3bf21e41022ac274c0fe42dd4e78639e4

        let mut level = GDLevel::default();

        let mut parts = text.split('#');
        let main_part = parts.next().ok_or(GDApiFetchError::BoomlingsUnparsable)?;
        let user_part = parts.next().ok_or(GDApiFetchError::BoomlingsUnparsable)?;

        let mut user_id = None;
        let mut diff_num = None;
        let mut diff_denom = None;
        let mut auto = false;
        let mut is_demon = false;
        let mut demon_difficulty = None;

        main_part.split(':').array_chunks::<2>().for_each(|[k, v]| match k {
            "1" if let Ok(c) = v.parse() => level.id = c,
            "2" => level.name = v.try_into().unwrap_or_else(|_| "Unknown".try_into().unwrap()),
            "6" if let Ok(c) = v.parse::<i32>() => user_id = Some(c),
            "8" if let Ok(c) = v.parse::<i32>() => diff_num = Some(c),
            "9" if let Ok(c) = v.parse::<i32>() => diff_denom = Some(c),
            "17" if v == "1" => is_demon = true,
            "25" if v == "1" => auto = true,
            "43" if let Ok(c) = v.parse::<i32>() => demon_difficulty = Some(c),
            _ => {}
        });

        if let Some(num) = diff_num
            && let Some(denom) = diff_denom
            && let Some(demon) = demon_difficulty
        {
            level.difficulty = Self::calc_difficulty(auto, num, denom, is_demon, demon)
        };

        let Some(user_id) = user_id else {
            return Err(GDApiFetchError::BoomlingsUnparsable);
        };

        // User part looks like
        // user_id:username:account_id
        user_part.split('|').for_each(|segment| {
            let mut split = segment.split(':');
            if let Some(id) = split.next().and_then(|v| v.parse::<i32>().ok()) {
                if id == user_id {
                    level.author_name =
                        split.next().and_then(|v| v.try_into().ok()).unwrap_or_default();
                    level.author_id = split.next().and_then(|v| v.parse().ok()).unwrap_or(-1);
                }
            };
        });

        if level.id <= 0
            || level.name.is_empty()
            || level.author_id <= 0
            || level.author_name.is_empty()
        {
            return Err(GDApiFetchError::BoomlingsUnparsable);
        }

        // finally

        Ok(Some(level))
    }

    fn calc_difficulty(
        auto: bool,
        num: i32,
        denom: i32,
        is_demon: bool,
        demon: i32,
    ) -> GDDifficulty {
        if auto {
            return GDDifficulty::Auto;
        }

        if denom == 0 {
            return GDDifficulty::NA;
        }

        if is_demon {
            let mut fixed = demon.clamp(0, 6);
            if fixed != 0 {
                fixed -= 2;
            }

            GDDifficulty::new(6 + fixed)
        } else {
            let val = (num / denom).clamp(-1, 10);
            GDDifficulty::new(val)
        }
    }
}
