//! Credits, top-ups, invoices, the billing profile, auto-reload, the hAlpha purchase, and
//! the gamma conversion.

use crate::routes::Route;

routes! {
    /// The credit balance.
    BALANCE: 0 = Route::get("/api/billing/credits/balance/").scoped().billing();

    /// The credit transactions.
    TRANSACTIONS: 0 = Route::get("/api/billing/transactions/").own_account().paged();

    /// The invoices.
    INVOICES: 0 = Route::get("/api/billing/invoices/").scoped().billing().paged();

    /// One invoice as a PDF.
    INVOICE_PDF: 1 = Route::get("/api/billing/invoices/{invoice_id}/pdf/").scoped().billing();

    /// The billing profile.
    PROFILE: 0 = Route::get("/api/billing/profile/").scoped().billing();

    /// Replaces the billing profile.
    PROFILE_UPDATE: 0 = Route::put("/api/billing/profile/").scoped().billing();

    /// The auto-reload setting.
    AUTO_RELOAD: 0 = Route::get("/api/billing/auto-reload/").scoped().billing();

    /// Replaces the auto-reload setting.
    AUTO_RELOAD_UPDATE: 0 = Route::put("/api/billing/auto-reload/").scoped().billing();

    /// The auto-reload charges.
    AUTO_RELOAD_HISTORY: 0 = Route::get("/api/billing/auto-reload/history/").scoped().billing();

    /// Starts saving a card for auto-reload.
    AUTO_RELOAD_CARD_SETUP: 0 = Route::post("/api/billing/auto-reload/card-setup/")
        .scoped()
        .billing();

    /// The recurring card subscription.
    STRIPE_SUBSCRIPTION: 0 = Route::get("/api/billing/stripe/active-subscription/")
        .own_account()
        .beside_billing()
        .billing();

    /// Starts a card top-up.
    CARD_CHECKOUT: 0 = Route::post("/api/billing/stripe/create-checkout-session/")
        .scoped()
        .billing();

    /// Starts a bitcoin top-up.
    BITCOIN_INVOICE: 0 = Route::post("/api/billing/btcpay/create-invoice/").scoped().billing();

    /// The hAlpha price quote.
    NATIVE_QUOTE: 0 = Route::get("/api/billing/native/quote/").own_account();

    /// Buys credits with hAlpha.
    NATIVE_BUY: 0 = Route::post("/api/billing/native/buy/").own_account();

    /// The hAlpha purchases.
    NATIVE_PURCHASES: 0 = Route::get("/api/billing/native/purchases/").own_account();

    /// The gamma conversion setup.
    GAMMA: 0 = Route::get("/api/billing/gamma/").own_account();

    /// The gamma conversions.
    GAMMA_CONVERSIONS: 0 = Route::get("/api/billing/gamma/conversions/").own_account().paged();

    /// One gamma conversion.
    GAMMA_CONVERSION: 1 = Route::get("/api/billing/gamma/conversions/{conversion_id}/")
        .own_account();

    /// Converts gamma to credits.
    GAMMA_CONVERSION_CREATE: 0 = Route::post("/api/billing/gamma/conversions/").own_account();

    /// The challenge a hotkey signs to be linked.
    GAMMA_HOTKEY_CHALLENGE: 0 = Route::post("/api/billing/gamma/hotkeys/challenge/").own_account();

    /// Links a hotkey.
    GAMMA_HOTKEY_CREATE: 0 = Route::post("/api/billing/gamma/hotkeys/").own_account();

    /// Unlinks a hotkey.
    GAMMA_HOTKEY_DELETE: 1 = Route::delete("/api/billing/gamma/hotkeys/{hotkey}/").own_account();
}
