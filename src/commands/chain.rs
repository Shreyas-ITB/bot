use std::collections::HashMap;

use chrono::{DateTime, Utc};
use poise::serenity_prelude::Colour;
use serde::Deserialize;
use tracing::{debug, instrument};
use uuid::Uuid;
use vrsc::Amount;
use vrsc_rpc::RpcApi;

use crate::{Context, Error};

/// Show information about Verus blockchain.
#[instrument(skip(ctx), fields(request_id = %Uuid::new_v4() ))]
#[poise::command(track_edits, slash_command, category = "Miscellaneous")]
pub async fn chaininfo(ctx: Context<'_>) -> Result<(), Error> {
    let client = ctx.data().verus()?;
    let blockchain_info = client.get_blockchain_info()?;
    let mining_info = client.get_mining_info()?;

    let testnet_name = match ctx.data().settings.application.testnet {
        true => "Verus (testnet)",
        false => "Verus",
    };

    ctx.send(|reply| {
        reply
            .embed(|embed| {
                embed
                    .title(format!("{} info", testnet_name))
                    .field("height", blockchain_info.blocks, false)
                    .field("difficulty", blockchain_info.difficulty, false)
                    .field(
                        "amount staking",
                        Amount::from_vrsc(mining_info.stakingsupply).unwrap(),
                        false,
                    )
                    .field(
                        "average block fees",
                        Amount::from_vrsc(mining_info.averageblockfees).unwrap(),
                        false,
                    )
            })
            .ephemeral(true)
    })
    .await?;

    Ok(())
}

/// Shows the ip addresses of all the peers that are connected to the bot.
#[instrument(skip(ctx), fields(request_id = %Uuid::new_v4() ))]
#[poise::command(slash_command, category = "Miscellaneous")]
pub async fn peerinfo(ctx: Context<'_>) -> Result<(), Error> {
    let client = &ctx.data().verus()?;

    let peer_info = client
        .get_peer_info()?
        .into_iter()
        .filter(|peer| peer.inbound == false)
        .collect::<Vec<_>>();

    ctx.send(|reply| {
        reply.ephemeral(true).content(format!(
            "Publicly available peers:```{}```",
            peer_info
                .into_iter()
                .map(|peer| peer.addr)
                .collect::<Vec<_>>()
                .join("\n"),
        ))
    })
    .await?;

    Ok(())
}

/// Show VRSC price information
#[instrument(skip(ctx), fields(request_id = %Uuid::new_v4() ))]
#[poise::command(slash_command, category = "Miscellaneous")]
pub async fn price(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer().await?;

    let resp: CoinPaprika =
        reqwest::get("https://api.coinpaprika.com/v1/tickers/vrsc-verus-coin?quotes=USD,BTC")
            .await?
            .json()
            .await?;

    let btc_price = resp
        .quotes
        .get("BTC")
        .and_then(|obj| Some(obj.price))
        .unwrap_or(0.0);

    let usd_price = resp
        .quotes
        .get("USD")
        .and_then(|obj| Some(obj.price))
        .unwrap_or(0.0);

    let usd_volume = resp
        .quotes
        .get("USD")
        .and_then(|obj| Some(obj.volume_24h))
        .unwrap_or(0.0);

    let price_up = resp
        .quotes
        .get("BTC")
        .and_then(|obj| Some(obj.percent_change_24h))
        .unwrap_or(0.0)
        .is_sign_positive();

    ctx.send(|reply| {
        reply.embed(|embed| {
            embed
                .title("VRSC price information")
                .field("USD price", format!("$ {:.4} ", &usd_price), true)
                .field("BTC price", format!("₿ {:.8} ", &btc_price), true)
                .field(
                    "% from ATH (USD)",
                    resp.quotes
                        .get("USD")
                        .and_then(|obj| Some(obj.percent_from_price_ath))
                        .unwrap_or(0.0),
                    false,
                )
                .field("Volume 24h (USD)", format!("{:.8}", &usd_volume), false)
                .field(
                    "Circulating supply (VRSC)",
                    format!("{}", resp.circulating_supply),
                    false,
                )
                .timestamp(resp.last_updated)
                .color(match price_up {
                    true => Colour::DARK_GREEN,
                    false => Colour::RED,
                })
                .footer(|footer| {
                    footer
                        .text("Data from CoinPaprika")
                        .icon_url("https://i.imgur.com/wwH60Uf.png")
                })
        })
    })
    .await?;

    Ok(())
}

/// Show currency information
#[instrument(skip(ctx), fields(request_id = %Uuid::new_v4() ))]
#[poise::command(slash_command, category = "Miscellaneous")]
pub async fn currency(ctx: Context<'_>, currency: String) -> Result<(), Error> {
    let verus_client = ctx.data().verus()?;
    let price: CoinPaprika =
        reqwest::get("https://api.coinpaprika.com/v1/tickers/vrsc-verus-coin?quotes=USD,BTC")
            .await?
            .json()
            .await?;

    let usd_price = price
        .quotes
        .get("USD")
        .and_then(|obj| Some(obj.price))
        .unwrap_or(0.0);

    let mut fields = vec![];

    if let Ok(currency) = verus_client.get_currency(&currency) {
        if let Some(currency_state) = currency.bestcurrencystate {
            fields.push((
                "Supply",
                format!("{}", currency_state.supply.as_vrsc()),
                false,
            ));

            if let Some(reserve_currencies) = currency_state.reservecurrencies.as_ref() {
                debug!("{:#?}", &reserve_currencies);
                let mut baskets = reserve_currencies
                    .iter()
                    .filter_map(|rc| {
                        let name = ctx.data().to_currency_name(&rc.currencyid).ok()?;
                        Some((name, rc.reserves.as_vrsc()))
                    })
                    .collect::<Vec<(String, f64)>>();

                let longest_name_len = baskets.iter().max_by_key(|x| x.0.len()).unwrap().0.len();
                let longest_value_len = format!(
                    "{}",
                    baskets
                        .iter()
                        .map(|t| t.1 * 100_000_000.0)
                        .reduce(|acc, amount| amount.max(acc))
                        .unwrap()
                )
                .len();

                debug!("{longest_value_len}");

                baskets.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

                let tvl_str = format!(
                    "```{}```",
                    baskets
                        .iter()
                        .map(|tvl| format!(
                            "{name:<max_name_len$}: {value:>max$.*}",
                            8,
                            name = tvl.0,
                            value = tvl.1,
                            max_name_len = longest_name_len + 1,
                            max = longest_value_len + 1
                        ))
                        .collect::<Vec<_>>()
                        .join("\n")
                );

                println!("{}", tvl_str);

                fields.push(("Baskets", tvl_str, false));

                // divide supply by the lastconversionprice of verus
                if ctx.data().settings.application.testnet {
                    let price = reserve_currencies
                        .iter()
                        .find(|c| c.currencyid.to_string() == "iJhCezBExJHvtyH3fGhNnt2NhU4Ztkf2yq")
                        .and_then(|c| Some(c.priceinreserve))
                        .unwrap_or(Amount::ZERO);

                    let vrsc_value_of_currency_supply =
                        currency_state.supply.as_vrsc() * price.as_vrsc(); //.unwrap_or(Amount::ZERO)

                    let dollar_value_of_currency_supply =
                        dbg!(vrsc_value_of_currency_supply * usd_price);

                    fields.push((
                        "est. currency value (USD)",
                        format!("$ {dollar_value_of_currency_supply:.2}"),
                        false,
                    ));
                }
            }

            ctx.send(|reply| {
                reply.embed(|embed| {
                    embed
                        .title(format!("{}", currency.fullyqualifiedname))
                        .fields(fields)
                })
            })
            .await?;
        }
    }

    Ok(())
}

#[derive(Deserialize, Debug)]
pub struct CoinPaprika {
    #[serde(rename = "id")]
    pub guid: String,
    pub symbol: String,
    pub circulating_supply: u64,
    pub last_updated: DateTime<Utc>,
    pub quotes: HashMap<String, CoinPaprikaQuoteCoin>,
}

#[derive(Deserialize, Debug)]
pub struct CoinPaprikaQuoteCoin {
    pub price: f64,
    pub volume_24h: f64,
    pub percent_change_24h: f64,
    pub percent_from_price_ath: f64,
}
