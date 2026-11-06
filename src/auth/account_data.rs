use server_shared::data::ClientAccountData;

#[derive(Clone)]
pub enum LoginKind<'a> {
    // TODO: deprecated
    UserToken(i32, &'a str),
    // TODO: deprecated
    Argon(i32, &'a str),
    ArgonV2(i32, &'a str),
    Plain(ClientAccountData),
}

impl LoginKind<'_> {
    pub fn account_id(&self) -> i32 {
        match self {
            LoginKind::UserToken(account_id, _) => *account_id,
            LoginKind::Argon(account_id, _) => *account_id,
            LoginKind::ArgonV2(account_id, _) => *account_id,
            LoginKind::Plain(data) => data.account_id,
        }
    }
}
