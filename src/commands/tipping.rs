use ::chrono::Duration;
use poise::serenity_prelude::{self, CacheHttp, ChannelId, ReactionType, RoleId, UserId};

use sqlx::{types::chrono, PgPool};
use tracing::*;
use uuid::Uuid;
use vrsc::Amount;

use crate::{
    commands::{misc::Notification, user_blacklisted},
    util::database::{self},
    wallet::get_and_check_balance,
    Context, Error,
};

/// Tip a user or a role
///
/// -------- :robot: **Tipping a user** --------
/// Tip a role by entering and selecting the user name. The selection menu will update as you type.
///
/// -------- :robot: **Tipping a role** --------
/// Tip a role by entering and selecting the role name. The role name can be any role, even the @everyone role. \
/// The amount entered in the second parameter will be split evenly among the members of the role.
#[instrument(skip(_ctx), fields(request_id = %Uuid::new_v4() ))]
#[poise::command(slash_command, category = "Tipping", subcommands("role", "user"))]
pub async fn tip(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Tip a role by entering and selecting the role name.
#[instrument(skip(ctx), fields(request_id = %Uuid::new_v4() ))]
#[poise::command(slash_command, category = "Tipping")]
async fn role(
    ctx: Context<'_>,
    #[description = "Enter and select the role you want to tip"] role: serenity_prelude::Role,
    #[description = "The amount you want to tip"]
    #[min = 0.5]
    tip_amount: f64,
) -> Result<(), Error> {
    if user_blacklisted(ctx, ctx.author().id).await? {
        return Ok(());
    }

    debug!("role: {:?}", role.id);
    let tip_amount = Amount::from_vrsc(tip_amount)?;

    if get_and_check_balance(&ctx, tip_amount, Amount::ZERO)
        .await?
        .is_some()
    {
        trace!("tipper has enough balance");

        if let Some(guild) = ctx.guild() {
            debug!("guildid: {:?}", guild.id);
            let guild_members = guild.members.values();
            let role_members = guild_members
                .filter(
                    |m| m.roles.contains(&role.id) || &role.id == &RoleId(guild.id.0), // @everyone role_id (same as guild_id) does never get tips
                )
                .map(|m| m.user.id)
                .collect::<Vec<_>>();

            tip_multiple_users(
                &ctx.data().database,
                ctx.author().id,
                ctx.http(),
                &ctx.channel_id(),
                &role_members,
                &tip_amount,
                "role",
            )
            .await?;

            return Ok(());
        } else {
            trace!("not in a guild, send error");

            ctx.send(|reply| {
                reply.ephemeral(true).content(format!(
                    "You need to be in a Discord server to use this command."
                ))
            })
            .await?;

            return Ok(());
        }
    }

    Ok(())
}

/// Tip a user by entering and selecting the user's name.
#[instrument(skip(ctx), fields(request_id = %Uuid::new_v4() ))]
#[poise::command(slash_command, category = "Tipping")]
async fn user(
    ctx: Context<'_>,
    #[description = "Enter and select the user you want to tip"] user: serenity_prelude::User,
    #[description = "The amount you want to tip"] tip_amount: f64,
) -> Result<(), Error> {
    if user_blacklisted(ctx, ctx.author().id).await? {
        return Ok(());
    }

    let tip_amount = Amount::from_vrsc(tip_amount)?;

    debug!(
        "user {} ({}) wants to tip {} with {tip_amount}",
        ctx.author().name,
        ctx.author().id,
        user.id
    );

    // check if the tipper has enough balance
    // update both balances in 1 go

    let pool = &ctx.data().database;

    if get_and_check_balance(&ctx, tip_amount, Amount::ZERO)
        .await?
        .is_some()
    {
        trace!("tipper has enough balance");

        database::process_a_tip(pool, &ctx.author().id, &vec![user.id], &tip_amount).await?;

        // tips are only stored one way: counterparty is the sender of the tip.
        let tip_event_id = Uuid::new_v4();
        database::store_tip_transactions(
            pool,
            &tip_event_id,
            &vec![user.id],
            "direct",
            &tip_amount,
            ctx.author().id,
        )
        .await?;

        match database::get_notification_settings(&pool, &vec![user.id])
            .await?
            .first()
        {
            Some((_, notification)) => {
                match notification {
                    Notification::All | Notification::ChannelOnly => {
                        // send a message in the same channel:
                        ctx.send(|reply| {
                            reply.ephemeral(false).content(format!(
                                "<@{}> just tipped <@{}> {tip_amount}!",
                                &ctx.author().id,
                                user.id
                            ))
                        })
                        .await?;
                    }
                    Notification::DMOnly => {
                        // send a non-pinging message in the channel:
                        ctx.send(|reply| {
                            reply.ephemeral(false).content(format!(
                                "<@{}> just tipped `{}` {tip_amount}!",
                                &ctx.author().id,
                                user.tag()
                            ))
                        })
                        .await?;
                        // send a notification in dm:
                        user.dm(&ctx.http(), |message| {
                            message.content(format!(
                                "You just got tipped {tip_amount} from <@{}>!",
                                &ctx.author().id,
                            ))
                        })
                        .await?;
                    }
                    Notification::Off => {
                        // send a non-pinging message in the channel:
                        ctx.send(|reply| {
                            reply.ephemeral(false).content(format!(
                                "<@{}> just tipped `{}` {tip_amount}!",
                                &ctx.author().id,
                                user.tag()
                            ))
                        })
                        .await?;
                    }
                }
            }
            None => {
                trace!("User has not set notification settings, defaulting to Channel");

                ctx.send(|reply| {
                    reply.ephemeral(false).content(format!(
                        "<@{}> just tipped <@{}> {tip_amount}!",
                        &ctx.author().id,
                        user.id
                    ))
                })
                .await?;
            }
        }

        return Ok(());
    }

    Ok(())
}

#[derive(Debug, poise::ChoiceParameter)]
pub enum Hms {
    Hours,
    Minutes,
}

/// Start a giveaway where users need to react to a message to participate
///
/// -------- :robot: **Reactdrop** --------
/// When initiating a reactdrop, find a suitable emoji in the first parameter. \
/// It can be any Emoji, as long as the emoji is in the current server.
///
/// The amount is entered in the second parameter. This amount will be split among the participants of the reactdrop when it ends.
#[instrument(skip(ctx), fields(request_id = %Uuid::new_v4() ))]
#[poise::command(slash_command, category = "Tipping")]
pub async fn reactdrop(
    ctx: Context<'_>,
    #[description = "The emoji users need to react with"] emoji: String,
    #[min = 0.1]
    #[description = "The amount you want to give away"]
    amount: f64,
    #[min = 1] time: i64,
    #[description = "The time in hours, minutes or seconds"] hms: Hms,
) -> Result<(), Error> {
    if user_blacklisted(ctx, ctx.author().id).await? {
        return Ok(());
    }

    let tip_amount = Amount::from_vrsc(amount)?;

    if get_and_check_balance(&ctx, tip_amount, Amount::ZERO)
        .await?
        .is_some()
    {
        debug!("emoji picked for reactdrop: {}", emoji);

        if let Ok(reaction_type) = ReactionType::try_from(emoji) {
            match &reaction_type {
                ReactionType::Custom { id, .. } => {
                    let emojis = ctx.guild().unwrap().emojis(ctx.http()).await?;
                    if !emojis.iter().any(|e| e.id == id.0) {
                        trace!("emoji not in guild");
                        ctx.send(|reply| {
                            reply.ephemeral(true).content("This emoji is not found in this Discord server, so it can't be used. Please pick another one")
                        }).await?;

                        return Ok(());
                    } else {
                        debug!("emoji in guild");
                    }
                }
                ReactionType::Unicode(unicode) => {
                    let emoji = emojis::get(&unicode);

                    if emoji.is_none() {
                        ctx.send(|reply| {
                            reply.ephemeral(true).content(
                                "This is not a valid emoji. Please pick an emoji to start a Reactdrop",
                            )
                        })
                        .await?;

                        return Ok(());
                    } else {
                        trace!("valid unicode");
                    }
                }
                ref s => {
                    unreachable!("we find ourselves in a weird state: {:?}", s);
                }
            }

            trace!("valid emoji");

            let time_in_seconds: Duration = match hms {
                Hms::Hours => Duration::seconds(time * 60 * 60),
                Hms::Minutes => Duration::seconds(time * 60),
            };

            let now = chrono::Utc::now();
            let finish_time = now.checked_add_signed(time_in_seconds).unwrap(); // sane values are guaranteed by command argument limits
            debug!("finish_time: {finish_time:?}");

            let reply_handle = ctx
                .say(format!(
                    ">>> **A reactdrop of {tip_amount} was started!**\n\n \
React with the {} emoji to participate\n\nTime remaining: {} hour(s) and {} minute(s)",
                    reaction_type.clone(),
                    time_in_seconds.num_seconds() / (60 * 60),
                    (time_in_seconds.num_seconds() / 60) % 60
                ))
                .await?;
            let msg = reply_handle.into_message().await?;
            msg.react(ctx.http(), reaction_type.clone()).await?;

            // a reactdrop can be started for as long as a user wants it to last. Discord however limits the lifetime of a context to 15 minutes.
            // We must account for this by extracting the necessary data from `Context` and store it for later use.
            let channel_id = ctx.channel_id();
            let message_id = msg.id;

            database::insert_reactdrop(
                &ctx.data().database,
                ctx.author().id.try_into()?,
                reaction_type.to_string(),
                Amount::from_vrsc(amount).unwrap().as_sat() as i64,
                channel_id.try_into()?,
                message_id.try_into()?,
                finish_time,
            )
            .await?;
        }
    }

    Ok(())
}

// Divides the amount over the `users` vec, increases the balance for all `users` and stores the tip transaction
// This function gets called in `tip role` and `reactdrop`
// We need the ChannelId here because ReactDrops tend to last longer than 15 minutes, which is the time Discord drops the context, giving
// us an invalid webhook token when trying to send a message using that context.
pub async fn tip_multiple_users(
    pool: &PgPool,
    author: UserId,
    http: impl CacheHttp + std::convert::AsRef<poise::serenity_prelude::Http>,
    channel_id: &ChannelId,
    users: &Vec<UserId>,
    amount: &Amount,
    kind: &str,
) -> Result<(), Error> {
    // TODO optimize this query (select all that don't exist, insert them in 1 go)
    // check if all the tippees have an entry in the db
    // let pool = &ctx.data().database;
    // let author = ctx.author().id;
    // let http = ctx.http();

    debug!("users in tip_users: {:?}", users);

    // need to divide tipping amount over number of users
    if let Some(div_tip_amount) = amount.checked_div(users.len() as u64) {
        let amount = div_tip_amount
            .checked_mul(users.len() as u64)
            .unwrap_or(*amount);
        debug!("after division every member gets {div_tip_amount}");
        debug!("members: {:#?}", &users);

        let tip_event_id = Uuid::new_v4();

        database::process_a_tip(pool, &author, &users, &div_tip_amount).await?;

        database::store_tip_transactions(pool, &tip_event_id, users, kind, &div_tip_amount, author)
            .await?;

        let notification_settings = database::get_notification_settings(pool, &users).await?;

        for (user_id, notification) in notification_settings {
            match (user_id, notification) {
                (_, Notification::All) | (_, Notification::DMOnly) => {
                    let user = UserId(user_id as u64).to_user(&http).await?;
                    user.dm(&http, |message| {
                        message.content(format!(
                            "You just got tipped {div_tip_amount} from <@{}>!",
                            &author,
                        ))
                    })
                    .await?;
                }
                _ => {
                    // don't ping when ChannelOnly or Off
                }
            }
        }

        channel_id
            .send_message(http, |message| {
                message.content(format!(
                    "<@{}> just tipped {} to {} users!",
                    &author,
                    amount,
                    &users.len()
                ))
            })
            .await?;
    } else {
        error!("could not send tip to role");
    }

    Ok(())
}
