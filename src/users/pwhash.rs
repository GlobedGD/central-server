pub fn hash(password: &str) -> String {
    bcrypt::hash(password, 8).expect("failed to hash password")
}

pub fn verify(password: &str, hash: &str) -> bool {
    bcrypt::verify(password, hash).unwrap_or(false)
}
