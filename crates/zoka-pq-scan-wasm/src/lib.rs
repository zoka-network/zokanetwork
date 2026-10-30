//! A wallet's balance from its view key, inside the browser.
//!
//! The page fetches the node's public `/scan` pages, in order, and hands their
//! bytes to a [`Scanner`]; the key stays in this module's memory and is wiped
//! when the scanner is freed. The node only ever sees requests for pages that
//! every visitor downloads alike.
#![forbid(unsafe_code)]
use wasm_bindgen::prelude::*;
use zoka_pq_pool::{
    scan::{ScanPage, Viewer},
    wallet::ViewKey,
};

#[wasm_bindgen]
pub struct Scanner {
    viewer: Viewer,
}

#[wasm_bindgen]
impl Scanner {
    /// `network`: 1 mainnet, 2 testnet, 3 regtest.
    #[wasm_bindgen(constructor)]
    pub fn new(view_key: &str, network: u8) -> Result<Scanner, JsError> {
        let key = ViewKey::decode(view_key, network)
            .map_err(|_| JsError::new("that is not a view key of this network"))?;
        Ok(Self {
            viewer: Viewer::new(key),
        })
    }

    /// The short address whose payments this key sees.
    pub fn address(&self) -> Result<String, JsError> {
        self.viewer
            .key()
            .address()
            .short()
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// The height of the next block it needs: ask for `/scan?from=<next>`.
    pub fn next(&self) -> u32 {
        self.viewer.next()
    }

    /// Read one page (it has to start at [`Scanner::next`]); returns the tip
    /// the node had when it served it.
    pub fn add_page(&mut self, bytes: &[u8]) -> Result<u32, JsError> {
        let page = ScanPage::decode(bytes)
            .map_err(|_| JsError::new("the node sent an unreadable page"))?;
        self.viewer
            .add_page(&page)
            .map_err(|_| JsError::new("that page does not follow the last one read"))?;
        Ok(page.tip)
    }

    /// Balance and history at the last block read, as JSON (atoms).
    pub fn balance(&self) -> String {
        serde_json::to_string(&self.viewer.balance()).unwrap_or_else(|_| "null".into())
    }
}
