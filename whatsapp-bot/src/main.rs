use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use wacore::types::events::Event;
use wacore_binary::jid::Jid;
use waproto::whatsapp as wa;
use whatsapp_rust::bot::Bot;
use whatsapp_rust::pair_code::PairCodeOptions;
use whatsapp_rust::store::SqliteStore;
use whatsapp_rust::Client;
use whatsapp_rust_tokio_transport::TokioWebSocketTransportFactory;
use whatsapp_rust_ureq_http_client::UreqHttpClient;

/// Equipment item with name and optional quantity
#[derive(Clone, Debug)]
struct EquipmentItem {
    name: String,
    quantity: u32,
}

/// Equipment list storage - maps chat JID to list of equipment
type EquipmentStore = Arc<RwLock<HashMap<String, Vec<EquipmentItem>>>>;

/// Parse a command from message text
/// Commands start with /sh or /slatehub and are case-insensitive
fn parse_command(text: &str) -> Option<(&str, &str)> {
    let text = text.trim();
    let text_lower = text.to_lowercase();

    // Check for /slatehub or /sh prefix
    let rest = if text_lower.starts_with("/slatehub ") {
        &text[10..] // Remove "/slatehub "
    } else if text_lower.starts_with("/sh ") {
        &text[4..] // Remove "/sh "
    } else if text_lower == "/slatehub" || text_lower == "/sh" {
        // Just the prefix with no command = help
        return Some(("help", ""));
    } else {
        return None;
    };

    let mut parts = rest.splitn(2, |c: char| c.is_whitespace());
    let command = parts.next()?;
    let args = parts.next().unwrap_or("").trim();
    Some((command, args))
}

/// Process equipment commands and return response text
async fn process_command(
    command: &str,
    args: &str,
    chat_id: &str,
    store: &EquipmentStore,
) -> Option<String> {
    let command = command.to_lowercase();

    match command.as_str() {
        "help" | "equiphelp" => Some(
            "*SlateHub Equipment Bot Commands*\n\n\
                /sh add <item> [x quantity] - Add equipment\n\
                /sh remove <item> - Remove equipment\n\
                /sh list - Show all equipment\n\
                /sh clear - Clear all equipment\n\
                /sh update <item> x <quantity> - Update quantity\n\n\
                _Examples:_\n\
                /sh add ARRI Alexa Mini\n\
                /sh add C-Stand x 5\n\
                /sh update C-Stand x 10\n\
                /sh remove C-Stand\n\n\
                _You can also use /slatehub instead of /sh_"
                .to_string(),
        ),

        "add" => {
            if args.is_empty() {
                return Some(
                    "Usage: /sh add <item> [x quantity]\nExample: /sh add C-Stand x 5".to_string(),
                );
            }

            let (name, quantity) = parse_item_with_quantity(args);

            let mut store = store.write().await;
            let list = store.entry(chat_id.to_string()).or_insert_with(Vec::new);

            // Check if item already exists (case-insensitive)
            if let Some(existing) = list
                .iter_mut()
                .find(|i| i.name.to_lowercase() == name.to_lowercase())
            {
                existing.quantity += quantity;
                Some(format!(
                    "Updated *{}* quantity to {}",
                    existing.name, existing.quantity
                ))
            } else {
                list.push(EquipmentItem {
                    name: name.clone(),
                    quantity,
                });
                if quantity > 1 {
                    Some(format!("Added *{}* x {}", name, quantity))
                } else {
                    Some(format!("Added *{}*", name))
                }
            }
        }

        "remove" | "delete" | "rm" => {
            if args.is_empty() {
                return Some("Usage: /sh remove <item>\nExample: /sh remove C-Stand".to_string());
            }

            let mut store = store.write().await;
            let list = store.entry(chat_id.to_string()).or_insert_with(Vec::new);

            let args_lower = args.to_lowercase();
            if let Some(pos) = list
                .iter()
                .position(|i| i.name.to_lowercase() == args_lower)
            {
                let removed = list.remove(pos);
                Some(format!("Removed *{}*", removed.name))
            } else {
                Some(format!("Item '{}' not found in equipment list", args))
            }
        }

        "list" | "ls" | "equipment" => {
            let store = store.read().await;
            let list = store.get(chat_id);

            match list {
                Some(items) if !items.is_empty() => {
                    let mut response = "*Equipment List*\n\n".to_string();
                    for (i, item) in items.iter().enumerate() {
                        if item.quantity > 1 {
                            response.push_str(&format!(
                                "{}. {} x {}\n",
                                i + 1,
                                item.name,
                                item.quantity
                            ));
                        } else {
                            response.push_str(&format!("{}. {}\n", i + 1, item.name));
                        }
                    }
                    response.push_str(&format!("\n_Total: {} items_", items.len()));
                    Some(response)
                }
                _ => {
                    Some("Equipment list is empty.\nUse !add <item> to add equipment.".to_string())
                }
            }
        }

        "clear" | "reset" => {
            let mut store = store.write().await;
            store.remove(chat_id);
            Some("Equipment list cleared.".to_string())
        }

        "update" | "set" => {
            if args.is_empty() {
                return Some(
                    "Usage: /sh update <item> x <quantity>\nExample: /sh update C-Stand x 10"
                        .to_string(),
                );
            }

            let (name, quantity) = parse_item_with_quantity(args);

            let mut store = store.write().await;
            let list = store.entry(chat_id.to_string()).or_insert_with(Vec::new);

            let name_lower = name.to_lowercase();
            if let Some(existing) = list
                .iter_mut()
                .find(|i| i.name.to_lowercase() == name_lower)
            {
                existing.quantity = quantity;
                Some(format!(
                    "Updated *{}* quantity to {}",
                    existing.name, quantity
                ))
            } else {
                Some(format!(
                    "Item '{}' not found. Use !add to add new items.",
                    name
                ))
            }
        }

        _ => None, // Unknown command, don't respond
    }
}

/// Parse item name and optional quantity from args
/// Formats: "Item Name" or "Item Name x 5" or "Item Name x5"
fn parse_item_with_quantity(args: &str) -> (String, u32) {
    // Try to find "x <number>" or "x<number>" pattern at the end
    let args = args.trim();

    // Look for " x " or " x" followed by digits
    if let Some(x_pos) = args.to_lowercase().rfind(" x") {
        let (name_part, qty_part) = args.split_at(x_pos);
        let qty_str: String = qty_part.chars().filter(|c| c.is_ascii_digit()).collect();

        if let Ok(qty) = qty_str.parse::<u32>() {
            if qty > 0 {
                return (name_part.trim().to_string(), qty);
            }
        }
    }

    (args.to_string(), 1)
}

/// Send a reply message to a chat
async fn send_reply(
    client: &Arc<Client>,
    chat: &Jid,
    text: &str,
    reply_to_id: &str,
    reply_to_sender: &Jid,
    reply_to_msg: &wa::Message,
) {
    // Create a reply with quote
    let context_info = wa::ContextInfo {
        stanza_id: Some(reply_to_id.to_string()),
        participant: Some(reply_to_sender.to_string()),
        quoted_message: Some(Box::new(reply_to_msg.clone())),
        ..Default::default()
    };

    let message = wa::Message {
        extended_text_message: Some(Box::new(wa::message::ExtendedTextMessage {
            text: Some(text.to_string()),
            context_info: Some(Box::new(context_info)),
            ..Default::default()
        })),
        ..Default::default()
    };

    match client.send_message(chat.clone(), message).await {
        Ok(msg_id) => {
            println!("[REPLY] Sent successfully, msg_id={}", msg_id);
        }
        Err(e) => {
            println!("[ERROR] Failed to send reply: {:?}", e);
            error!("Failed to send reply: {:?}", e);
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load .env file from current directory, then parent directory
    dotenvy::dotenv().ok();

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("whatsapp_bot=debug".parse()?),
        )
        .init();

    info!("Starting SlateHub WhatsApp Bot...");

    // Check for phone number pairing option
    let phone_number = std::env::var("WHATSAPP_PHONE_NUMBER")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::args().nth(1));

    println!(
        "WHATSAPP_PHONE_NUMBER from env: {:?}",
        std::env::var("WHATSAPP_PHONE_NUMBER")
    );
    println!("Phone number to use: {:?}", phone_number);

    if let Some(ref phone) = phone_number {
        info!("Phone number pairing enabled for: {}", phone);
        println!("\n========================================");
        println!("Phone number pairing mode");
        println!("On your phone, go to:");
        println!("  WhatsApp > Linked Devices > Link a Device");
        println!("  > Link with phone number instead");
        println!("========================================\n");
    } else {
        println!("\n========================================");
        println!("QR code pairing mode (default)");
        println!("Tip: For phone number pairing instead, run:");
        println!("  WHATSAPP_PHONE_NUMBER=15551234567 cargo run");
        println!("  or: cargo run -- 15551234567");
        println!("========================================\n");
    }

    // Create SQLite storage for session persistence
    let db_path =
        std::env::var("WHATSAPP_DB_PATH").unwrap_or_else(|_| "whatsapp_session.db".to_string());

    info!("Using database: {}", db_path);

    let backend = Arc::new(
        SqliteStore::new(&db_path)
            .await
            .expect("Failed to create SQLite store"),
    );

    // Create shared equipment storage
    let equipment_store: EquipmentStore = Arc::new(RwLock::new(HashMap::new()));

    let mut builder = Bot::builder()
        .with_backend(backend)
        .with_transport_factory(TokioWebSocketTransportFactory::new())
        .with_http_client(UreqHttpClient::new());

    // Clone store for the event handler
    let store = equipment_store.clone();

    builder = builder.on_event(move |event, client| {
        let store = store.clone();
        async move {
            match event {
                Event::PairingQrCode { code, .. } => {
                    println!("\n========================================");
                    println!("Scan this QR code with WhatsApp:");
                    println!("========================================\n");
                    println!("{}", code);
                    println!("\n========================================\n");
                }

                Event::PairingCode { code, timeout } => {
                    println!("\n========================================");
                    println!("Enter this code on your phone:");
                    println!("  {}", code);
                    println!("Code expires in {:?}", timeout);
                    println!("========================================\n");
                }

                Event::Connected(_) => {
                    info!("Connected to WhatsApp!");
                    println!("\n*** Connected to WhatsApp! ***\n");
                }

                Event::PairSuccess(pair_info) => {
                    info!("Pairing successful! JID: {:?}", pair_info.id);
                    println!("\n*** Pairing successful! ***\n");
                }

                Event::LoggedOut(logout_info) => {
                    warn!("Logged out: {:?}", logout_info.reason);
                }

                Event::JoinedGroup(lazy_conv) => {
                    if let Some(conv) = lazy_conv.get() {
                        info!("Joined group: {}", conv.id);
                    }
                }

                Event::Message(msg, msg_info) => {
                    let sender = &msg_info.source.sender;
                    let chat = &msg_info.source.chat;
                    let is_from_me = msg_info.source.is_from_me;
                    let message_id = &msg_info.id;

                    // Extract text content from the message
                    let text = extract_message_text(&msg);

                    println!(
                        "[MESSAGE] from={:?} chat={:?} is_from_me={} msg_id={} text={:?}",
                        sender, chat, is_from_me, message_id, text
                    );

                    // Skip messages from ourselves to avoid loops
                    if is_from_me {
                        println!("[MESSAGE] Skipping own message");
                        return;
                    }

                    if let Some(text) = text {
                        let text_lower = text.to_lowercase();
                        let chat_id = chat.to_string();

                        // Check for commands first
                        if let Some((command, args)) = parse_command(&text) {
                            println!("[COMMAND] cmd={} args={}", command, args);

                            if let Some(response) =
                                process_command(command, args, &chat_id, &store).await
                            {
                                send_reply(&client, chat, &response, message_id, sender, &msg)
                                    .await;
                            }
                        }
                        // Check if message contains "slatehub" (case-insensitive)
                        else if text_lower.contains("slatehub") {
                            println!("[MATCH] Detected 'slatehub' mention! Sending reply...");
                            info!(
                                "Detected 'slatehub' mention from {:?} in {:?}",
                                sender, chat
                            );

                            send_reply(&client, chat, "is awesome", message_id, sender, &msg).await;
                        }
                    }
                }

                _ => {
                    debug!("Received event: {:?}", event);
                }
            }
        }
    });

    // Add phone number pairing if configured
    if let Some(phone) = phone_number {
        let device_name =
            std::env::var("WHATSAPP_DEVICE_NAME").unwrap_or_else(|_| "SlateHub Bot".to_string());

        println!("Device name: {}", device_name);

        builder = builder.with_pair_code(PairCodeOptions {
            phone_number: phone,
            platform_display: device_name,
            ..Default::default()
        });
    }

    let mut bot = builder.build().await?;

    info!("Bot initialized, starting...");
    bot.run().await?.await?;

    Ok(())
}

/// Extract text content from various message types
fn extract_message_text(msg: &wa::Message) -> Option<String> {
    // First check for simple conversation text
    if let Some(ref text) = msg.conversation {
        return Some(text.clone());
    }

    // Check for extended text message
    if let Some(ref ext) = msg.extended_text_message {
        if let Some(ref text) = ext.text {
            return Some(text.clone());
        }
    }

    None
}
