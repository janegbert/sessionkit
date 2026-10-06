// Deliberately separate from accounts::Customer: billing needs the legal identity on an invoice,
// accounts the person who logs in. Do not merge them.
pub struct Customer { pub id: u64, pub legal_name: String, pub vat_number: Option<String>, pub email: String }
