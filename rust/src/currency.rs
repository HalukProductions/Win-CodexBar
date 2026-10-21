//! Preferred display currencies and safe conversion of provider cost amounts.

use std::collections::HashMap;
use std::time::Duration;

/// One entry of the preferred-display-currency catalog.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurrencyInfo {
    pub code: &'static str,
    pub symbol: &'static str,
    /// Offline USD-pivot rate used until live rates are available.
    pub fallback_rate: f64,
}

const fn currency(code: &'static str, symbol: &'static str, fallback_rate: f64) -> CurrencyInfo {
    CurrencyInfo {
        code,
        symbol,
        fallback_rate,
    }
}

/// Single source of truth for picker order, picker symbols, and offline rates.
/// The frontend copy (`apps/desktop-tauri/src/lib/currencyCatalog.generated.ts`)
/// is checked against this table by the catalog sync test.
pub const CURRENCIES: &[CurrencyInfo] = &[
    currency("USD", "$", 1.0),
    currency("GBP", "£", 0.79),
    currency("EUR", "€", 0.92),
    currency("CZK", "Kč", 21.0),
    currency("CNY", "¥", 7.27),
    currency("JPY", "¥", 154.0),
    currency("KRW", "₩", 1428.90),
    currency("CAD", "$", 1.38),
    currency("AUD", "$", 1.55),
    currency("HKD", "$", 7.80),
    currency("TWD", "NT$", 32.30),
    currency("SGD", "$", 1.34),
    currency("INR", "₹", 84.50),
    currency("CHF", "Fr.", 0.80),
    currency("AED", "د.إ", 3.6725),
    currency("TRY", "₺", 48.5),
    // Rates below are from open.er-api.com on 2026-09-24.
    currency("NZD", "$", 1.761),
    currency("SEK", "kr", 9.908),
    currency("NOK", "kr", 9.480),
    currency("DKK", "kr", 6.554),
    currency("PLN", "zł", 3.838),
    currency("BRL", "R$", 5.117),
    currency("MXN", "$", 17.47),
    currency("ZAR", "R", 16.36),
    currency("THB", "฿", 33.37),
    currency("IDR", "Rp", 17836.0),
    currency("VND", "₫", 25962.0),
    currency("UAH", "₴", 44.86),
    // Rate from open.er-api.com on 2026-10-19.
    currency("ILS", "₪", 3.109),
];

pub fn is_supported_currency(code: &str) -> bool {
    CURRENCIES.iter().any(|currency| currency.code == code)
}

/// Renders the catalog as the TypeScript module the settings picker and
/// offline converter import, so both sides share this table. This is only
/// needed by the sync test and the opt-in regeneration path.
#[cfg(test)]
fn render_typescript_catalog() -> String {
    let mut out = String::from(
        "// Generated from rust/src/currency.rs (CURRENCIES). Do not edit by hand.\n\
         // Regenerate (PowerShell): $env:UPDATE_CURRENCY_CATALOG = '1'; cargo test -p codexbar currency_catalog\n\
         export const CURRENCY_CATALOG = [\n",
    );
    for currency in CURRENCIES {
        let code = serde_json::to_string(currency.code).expect("currency codes serialize");
        let symbol = serde_json::to_string(currency.symbol).expect("currency symbols serialize");
        out.push_str(&format!(
            "  {{ code: {code}, symbol: {symbol}, fallbackRate: {} }},\n",
            currency.fallback_rate
        ));
    }
    out.push_str("] as const;\n");
    out
}

/// The normalized display-currency preference. `AUTO` keeps every surface in
/// its source units; `Code` converts through the USD pivot. Serialized as the
/// same `AUTO` / ISO-code strings the settings file has always used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreferredCurrency {
    Auto,
    Code(&'static str),
}

impl PreferredCurrency {
    /// Normalize raw input: `AUTO` (any case) or a supported ISO code wins,
    /// anything else reads as `AUTO` (mirrors `normalize_preferred_currency`).
    pub fn parse(value: &str) -> Self {
        let code = normalize_preferred_currency(value);
        if code == "AUTO" {
            return Self::Auto;
        }
        Self::Code(
            CURRENCIES
                .iter()
                .map(|currency| currency.code)
                .find(|supported| *supported == code)
                .unwrap_or("USD"),
        )
    }

    /// The persisted form: `"AUTO"` or the ISO code.
    pub fn raw(&self) -> String {
        match self {
            Self::Auto => "AUTO".to_string(),
            Self::Code(code) => (*code).to_string(),
        }
    }

    pub fn is_auto(&self) -> bool {
        matches!(self, Self::Auto)
    }
}

pub fn normalize_preferred_currency(value: &str) -> String {
    let code = value.trim().to_ascii_uppercase();
    if code == "AUTO" || is_supported_currency(&code) {
        code
    } else {
        "AUTO".to_string()
    }
}

/// Keep only finite positive rates for supported codes, with USD pinned to
/// 1.0 (the conversion pivot). Empty when USD is absent or off — a rate table
/// without a correct USD pivot is unusable for conversion.
pub fn sanitize_rates(rates: &HashMap<String, f64>) -> HashMap<String, f64> {
    let mut clean = HashMap::new();
    for currency in CURRENCIES {
        if let Some(rate) = rates.get(currency.code).copied()
            && rate.is_finite()
            && rate > 0.0
        {
            clean.insert(currency.code.to_string(), rate);
        }
    }
    if clean
        .get("USD")
        .is_none_or(|rate| (*rate - 1.0).abs() > f64::EPSILON)
    {
        return HashMap::new();
    }
    clean
}

pub fn fallback_rates() -> HashMap<String, f64> {
    CURRENCIES
        .iter()
        .map(|currency| (currency.code.to_string(), currency.fallback_rate))
        .collect()
}

/// Parse ExchangeRate-API's USD response, retaining only finite positive rates
/// for currencies the display converter knows how to format.
pub fn parse_exchange_rates(payload: &[u8]) -> Option<HashMap<String, f64>> {
    let value: serde_json::Value = serde_json::from_slice(payload).ok()?;
    if value.get("result")?.as_str()? != "success" || value.get("base_code")?.as_str()? != "USD" {
        return None;
    }
    let rates = value.get("rates")?.as_object()?;
    let mut parsed = HashMap::new();
    for currency in CURRENCIES {
        let Some(rate) = rates.get(currency.code).and_then(serde_json::Value::as_f64) else {
            continue;
        };
        if rate.is_finite() && rate > 0.0 {
            parsed.insert(currency.code.to_string(), rate);
        }
    }
    if parsed
        .get("USD")
        .is_none_or(|rate| (*rate - 1.0).abs() > f64::EPSILON)
    {
        return None;
    }
    Some(parsed)
}

/// Convert between ISO currencies through USD. Unknown/non-ISO units and
/// unavailable or malformed rates return `None` so callers keep source data.
pub fn convert_amount(
    amount: f64,
    source_code: &str,
    target_code: &str,
    rates: &HashMap<String, f64>,
) -> Option<f64> {
    if !amount.is_finite() {
        return None;
    }
    let source = source_code.trim().to_ascii_uppercase();
    let target = target_code.trim().to_ascii_uppercase();
    if !is_supported_currency(&source) || !is_supported_currency(&target) {
        return None;
    }
    if source == target {
        return Some(amount);
    }
    let source_rate = if source == "USD" {
        1.0
    } else {
        *rates.get(&source)?
    };
    let target_rate = if target == "USD" {
        1.0
    } else {
        *rates.get(&target)?
    };
    if !source_rate.is_finite()
        || source_rate <= 0.0
        || !target_rate.is_finite()
        || target_rate <= 0.0
    {
        return None;
    }
    let converted = (amount / source_rate) * target_rate;
    converted.is_finite().then_some(converted)
}

/// Fetches the shared USD pivot rates with the app's existing proxy setup.
pub async fn fetch_exchange_rates() -> Result<HashMap<String, f64>, String> {
    let builder = crate::core::apply_app_proxy(reqwest::Client::builder())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10));
    let client = builder.build().map_err(|error| error.to_string())?;
    let response = client
        .get("https://open.er-api.com/v6/latest/USD")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Exchange rate service returned HTTP {}",
            response.status()
        ));
    }
    let payload = response.bytes().await.map_err(|error| error.to_string())?;
    parse_exchange_rates(&payload).ok_or_else(|| "Invalid USD exchange-rate response".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_currency_defaults_and_rejects_unknown_values() {
        assert_eq!(normalize_preferred_currency(""), "AUTO");
        assert_eq!(normalize_preferred_currency(" auto "), "AUTO");
        assert_eq!(normalize_preferred_currency("try"), "TRY");
        assert_eq!(normalize_preferred_currency("BTC"), "AUTO");
    }

    #[test]
    fn parses_only_successful_usd_rates_that_are_finite_and_positive() {
        let rates = parse_exchange_rates(
            br#"{"result":"success","base_code":"USD","rates":{"USD":1,"TRY":48.5,"EUR":0.92,"GBP":0,"JPY":-1,"CAD":"bad","BTC":65000}}"#,
        )
        .unwrap();
        assert_eq!(rates.get("USD"), Some(&1.0));
        assert_eq!(rates.get("TRY"), Some(&48.5));
        assert!(!rates.contains_key("GBP"));
        assert!(!rates.contains_key("JPY"));
        assert!(!rates.contains_key("CAD"));
        assert!(!rates.contains_key("BTC"));
        assert!(
            parse_exchange_rates(br#"{"result":"error","base_code":"USD","rates":{"USD":1}}"#)
                .is_none()
        );
        assert!(
            parse_exchange_rates(br#"{"result":"success","base_code":"EUR","rates":{"USD":1}}"#)
                .is_none()
        );
    }

    #[test]
    fn typed_preference_round_trips_and_never_represents_invalid_codes() {
        assert_eq!(PreferredCurrency::parse(""), PreferredCurrency::Auto);
        assert_eq!(PreferredCurrency::parse(" auto "), PreferredCurrency::Auto);
        assert_eq!(
            PreferredCurrency::parse("try"),
            PreferredCurrency::Code("TRY")
        );
        assert_eq!(PreferredCurrency::parse("BTC"), PreferredCurrency::Auto);
        assert_eq!(PreferredCurrency::Code("TRY").raw(), "TRY");
        assert!(PreferredCurrency::Auto.is_auto());
        assert!(!PreferredCurrency::Code("TRY").is_auto());
    }

    #[test]
    fn sanitize_rates_pins_usd_and_drops_unsupported_or_invalid_entries() {
        let mut rates = fallback_rates();
        rates.insert("BTC".into(), 65000.0);
        rates.insert("TRY".into(), f64::NAN);
        rates.insert("USD".into(), 1.0);
        let clean = sanitize_rates(&rates);
        assert_eq!(clean.get("USD"), Some(&1.0));
        assert!(!clean.contains_key("BTC"));
        assert!(!clean.contains_key("TRY"));

        let mut broken = fallback_rates();
        broken.insert("USD".into(), 1.5);
        assert!(sanitize_rates(&broken).is_empty());
    }

    #[test]
    fn fallback_rates_and_usd_pivot_conversion_are_available_offline() {
        let rates = fallback_rates();
        assert_eq!(rates.get("TRY"), Some(&48.5));
        assert_eq!(convert_amount(10.0, "USD", "TRY", &rates), Some(485.0));
        let converted = convert_amount(10.0, "GBP", "TRY", &rates).unwrap();
        assert!((converted - 613.9240506329114).abs() < 1e-10);
    }

    #[test]
    fn unsupported_units_and_invalid_rates_never_convert() {
        let rates = fallback_rates();
        assert_eq!(convert_amount(12.0, "Credits", "TRY", &rates), None);
        assert_eq!(convert_amount(12.0, "USD", "BTC", &rates), None);
        let mut malformed = rates;
        malformed.insert("TRY".into(), f64::NAN);
        assert_eq!(convert_amount(12.0, "USD", "TRY", &malformed), None);
    }

    const UPSTREAM_ADDED: &[(&str, &str, f64)] = &[
        ("NZD", "$", 1.761),
        ("SEK", "kr", 9.908),
        ("NOK", "kr", 9.480),
        ("DKK", "kr", 6.554),
        ("PLN", "zł", 3.838),
        ("BRL", "R$", 5.117),
        ("MXN", "$", 17.47),
        ("ZAR", "R", 16.36),
        ("THB", "฿", 33.37),
        ("IDR", "Rp", 17836.0),
        ("VND", "₫", 25962.0),
        ("UAH", "₴", 44.86),
        ("ILS", "₪", 3.109),
    ];

    #[test]
    fn added_currencies_follow_try_in_upstream_order_with_upstream_rates() {
        let codes: Vec<&str> = CURRENCIES.iter().map(|currency| currency.code).collect();
        let try_index = codes.iter().position(|code| *code == "TRY").unwrap();
        assert_eq!(codes.len(), try_index + 1 + UPSTREAM_ADDED.len());
        for (offset, (code, symbol, rate)) in UPSTREAM_ADDED.iter().enumerate() {
            let entry = &CURRENCIES[try_index + 1 + offset];
            assert_eq!(
                (entry.code, entry.symbol, entry.fallback_rate),
                (*code, *symbol, *rate)
            );
        }
    }

    #[test]
    fn added_currencies_convert_through_the_usd_pivot_and_normalize() {
        let rates = fallback_rates();
        assert_eq!(rates.len(), CURRENCIES.len());
        for (code, _, rate) in UPSTREAM_ADDED {
            assert_eq!(normalize_preferred_currency(&code.to_lowercase()), *code);
            let from_usd = convert_amount(10.0, "USD", code, &rates).unwrap();
            assert!((from_usd - 10.0 * rate).abs() < 1e-9, "{code}");
            let back = convert_amount(from_usd, code, "USD", &rates).unwrap();
            assert!((back - 10.0).abs() < 1e-9, "{code}");
        }
        let cross = convert_amount(10.0, "EUR", "SEK", &rates).unwrap();
        assert!((cross - (10.0 / 0.92) * 9.908).abs() < 1e-9);
    }

    #[test]
    fn live_rate_parsing_keeps_added_currencies() {
        let rates = parse_exchange_rates(
            br#"{"result":"success","base_code":"USD","rates":{"USD":1,"VND":26000,"IDR":17900,"UAH":45,"XXX":3}}"#,
        )
        .unwrap();
        assert_eq!(rates.get("VND"), Some(&26000.0));
        assert_eq!(rates.get("IDR"), Some(&17900.0));
        assert_eq!(rates.get("UAH"), Some(&45.0));
        assert!(!rates.contains_key("XXX"));
    }

    #[test]
    fn currency_catalog_typescript_matches_the_checked_in_module() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../apps/desktop-tauri/src/lib/currencyCatalog.generated.ts");
        let expected = render_typescript_catalog();
        if std::env::var_os("UPDATE_CURRENCY_CATALOG").is_some() {
            std::fs::write(&path, &expected).unwrap();
        }
        let actual = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            actual.replace("\r\n", "\n"),
            expected,
            "set UPDATE_CURRENCY_CATALOG=1, then rerun `cargo test -p codexbar currency_catalog`"
        );
    }
}
