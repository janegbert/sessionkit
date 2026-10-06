pub mod persistence;
pub mod customer;
pub struct BillingGateway;
impl BillingGateway {
    pub fn create_invoice(&self, order_id: u64, total: u64) -> u64 { persistence::InvoiceRepository::insert(order_id, total) }
}
