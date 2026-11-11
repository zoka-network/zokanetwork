//! A wallet's receipts or balance from its view key, inside the browser.
//!
//! The page fetches the node's public `/scan` pages, in order, and hands their
//! bytes to a [`Scanner`]; the key stays in this module's memory and is wiped
//! when the scanner is freed. The node only ever sees requests for pages that
//! every visitor downloads alike.
#![forbid(unsafe_code)]
use wasm_bindgen::prelude::*;
use zoka_pq_pool::{
    scan::{IncomingViewer, ScanPage, Viewer},
    wallet::{IncomingViewKey, ViewKey},
};

enum Reader {
    Full(Viewer),
    Incoming(IncomingViewer),
}

#[wasm_bindgen]
pub struct Scanner {
    viewer: Reader,
}

#[wasm_bindgen]
impl Scanner {
    /// `network`: 1 mainnet, 2 testnet, 3 regtest.
    #[wasm_bindgen(constructor)]
    pub fn new(view_key: &str, network: u8) -> Result<Scanner, JsError> {
        let text = view_key.trim();
        let viewer = if let Ok(key) = IncomingViewKey::decode(text, network) {
            Reader::Incoming(IncomingViewer::new(key))
        } else if let Ok(key) = ViewKey::decode(text, network) {
            Reader::Full(Viewer::new(key))
        } else {
            return Err(JsError::new(
                "that is not an incoming or full view key of this network",
            ));
        };
        Ok(Self { viewer })
    }

    /// The short address whose payments this key sees.
    pub fn address(&self) -> Result<String, JsError> {
        match &self.viewer {
            Reader::Full(viewer) => viewer.key().address(),
            Reader::Incoming(viewer) => viewer.key().address(),
        }
        .short()
        .map_err(|e| JsError::new(&e.to_string()))
    }

    /// The height of the next block it needs: ask for `/scan?from=<next>`.
    pub fn next(&self) -> u32 {
        match &self.viewer {
            Reader::Full(viewer) => viewer.next(),
            Reader::Incoming(viewer) => viewer.next(),
        }
    }

    /// Read one page (it has to start at [`Scanner::next`]); returns the tip
    /// the node had when it served it.
    pub fn add_page(&mut self, bytes: &[u8]) -> Result<u32, JsError> {
        let page = ScanPage::decode(bytes)
            .map_err(|_| JsError::new("the node sent an unreadable page"))?;
        match &mut self.viewer {
            Reader::Full(viewer) => viewer.add_page(&page),
            Reader::Incoming(viewer) => viewer.add_page(&page),
        }
        .map_err(|_| JsError::new("that page does not follow the last one read"))?;
        Ok(page.tip)
    }

    /// JSON (atoms) at the last block read. Full keys retain the balance
    /// fields; incoming keys return receipts only, never a fabricated balance
    /// or a sent list. `kind` tells the page which fields it may display.
    pub fn balance(&self) -> String {
        let (kind, value) = match &self.viewer {
            Reader::Full(viewer) => ("full", serde_json::to_value(viewer.balance())),
            Reader::Incoming(viewer) => ("incoming", serde_json::to_value(viewer.receipts())),
        };
        let Ok(mut value) = value else {
            return "null".into();
        };
        value["kind"] = kind.into();
        value.to_string()
    }
}
