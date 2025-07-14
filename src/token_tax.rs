use ethers::types::H160;
use serde::Deserialize;
use dashmap::DashMap;

#[derive(Debug, Clone, Deserialize)]
pub struct TokenTaxInfo {
    #[serde(rename = "buyTax")]
    pub buy_tax: f64,
    #[serde(rename = "sellTax")]
    pub sell_tax: f64,
    #[serde(rename = "transferTax")]
    pub transfer_tax: f64,
    #[serde(rename = "simulationSuccess")]
    pub simulation_success: bool,
}

pub type TokenTaxMap = DashMap<H160, TokenTaxInfo>;

#[derive(Debug, Deserialize)]
struct TokenTaxInfoLine {
    #[serde(rename = "token")]
    token: String,
    #[serde(rename = "buyTax")]
    buy_tax: f64,
    #[serde(rename = "sellTax")]
    sell_tax: f64,
    #[serde(rename = "transferTax")]
    transfer_tax: f64,
    #[serde(rename = "simulationSuccess")]
    simulation_success: bool,
}

pub fn load_token_tax_map(path: &str) -> TokenTaxMap {
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    let file = File::open(path).expect("token_tax_report.jsonl not found");
    let reader = BufReader::new(file);
    let map = TokenTaxMap::new();

    for line in reader.lines() {
        if let Ok(line) = line {
            if let Ok(info) = serde_json::from_str::<TokenTaxInfoLine>(&line) {
                if let Ok(addr) = info.token.parse::<H160>() {
                    map.insert(addr, TokenTaxInfo {
                        buy_tax: info.buy_tax,
                        sell_tax: info.sell_tax,
                        transfer_tax: info.transfer_tax,
                        simulation_success: info.simulation_success,
                    });
                }
            }
        }
    }
    map
} 