#[derive(Clone, Copy, Debug)]
pub enum BotClientState {
    Started,
    SignedIn,
    CartUpdated,
    NotInStock,
    Purchased,
}
