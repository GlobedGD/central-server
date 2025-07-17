#[derive(Default, Debug)]
pub struct ClientAccountData {
    pub account_id: i32,
    pub user_id: i32,
    pub username: heapless::String<16>,
}

pub enum LoginKind<'a> {
    UserToken(i32, &'a str),
    Argon(i32, &'a str),
    Plain(ClientAccountData),
}
