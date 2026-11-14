use crate::infra::Db;

pub fn load(db: &Db) -> String {
    let _ = rand::random::<u8>();
    std::fs::read_to_string(&db.path).unwrap_or_default()
}
