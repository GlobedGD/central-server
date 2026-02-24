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
