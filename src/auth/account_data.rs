use server_shared::UsernameString;

#[derive(Default, Debug, Clone)]
pub struct ClientAccountData {
    pub account_id: i32,
    pub user_id: i32,
    pub username: UsernameString,
}

#[derive(Clone)]
pub enum LoginKind<'a> {
    UserToken(i32, &'a str),
    Argon(i32, &'a str),
    Plain(ClientAccountData),
}

impl LoginKind<'_> {
    pub fn account_id(&self) -> i32 {
        match self {
            LoginKind::UserToken(account_id, _) => *account_id,
            LoginKind::Argon(account_id, _) => *account_id,
            LoginKind::Plain(data) => data.account_id,
        }
    }
}
