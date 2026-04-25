/// ZOKA User-Friendly CLI for Wallets, Transactions, and Mining
use std::io::{self, Write};
use zoka::mining::UserMiner;
use zoka::transaction_pool::TransactionPool;
use zoka::wallet::{TransactionStatus, WalletManager};

fn main() {
    println!("🚀 Welcome to ZOKA Blockchain CLI");
    println!("=================================");
    println!();

    let mut wallet_manager = WalletManager::new();
    let mut transaction_pool = TransactionPool::new();
    let miner = UserMiner::new();

    loop {
        print_menu();
        let mut input = String::new();
        print!("\n> ");
        io::stdout().flush().unwrap();
        io::stdin().read_line(&mut input).unwrap();

        let command = input.trim();
        if command.is_empty() {
            continue;
        }

        match command {
            // Wallet commands
            "wallet create" => {
                print!("Enter wallet name: ");
                io::stdout().flush().unwrap();
                let mut name = String::new();
                io::stdin().read_line(&mut name).unwrap();
                let name = name.trim();

                match wallet_manager.create_wallet(name) {
                    Ok((address, mnemonic)) => {
                        println!("✅ Wallet created successfully!");
                        println!("📍 Address: {}", address);
                        println!("📝 Recovery Phrase (SAVE THIS SAFELY):");
                        println!("{}", mnemonic.join(" "));
                    }
                    Err(e) => println!("❌ Error: {}", e),
                }
            }

            "wallet import" => {
                print!("Enter wallet name: ");
                io::stdout().flush().unwrap();
                let mut name = String::new();
                io::stdin().read_line(&mut name).unwrap();
                let name = name.trim();

                print!("Enter mnemonic (space-separated): ");
                io::stdout().flush().unwrap();
                let mut mnemonic_str = String::new();
                io::stdin().read_line(&mut mnemonic_str).unwrap();
                let mnemonic: Vec<String> = mnemonic_str
                    .trim()
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect();

                match wallet_manager.import_wallet(name, &mnemonic) {
                    Ok(address) => {
                        println!("✅ Wallet imported successfully!");
                        println!("📍 Address: {}", address);
                    }
                    Err(e) => println!("❌ Error: {}", e),
                }
            }

            "wallet list" => {
                let wallets = wallet_manager.list_wallets();
                if wallets.is_empty() {
                    println!("📭 No wallets found. Create one with 'wallet create'");
                } else {
                    println!("📂 Your Wallets:");
                    for (name, address, balance) in &wallets {
                        println!("  • {} | {} | Balance: {} ZOKA", name, address, balance);
                    }
                }
            }

            "wallet balance" => {
                print!("Enter wallet name: ");
                io::stdout().flush().unwrap();
                let mut name = String::new();
                io::stdin().read_line(&mut name).unwrap();
                let name = name.trim();

                if let Some(wallet) = wallet_manager.get_wallet(name) {
                    println!("💰 Balance: {} ZOKA", wallet.balance);
                } else {
                    println!("❌ Wallet not found");
                }
            }

            "wallet export" => {
                print!("Enter wallet name: ");
                io::stdout().flush().unwrap();
                let mut name = String::new();
                io::stdin().read_line(&mut name).unwrap();
                let name = name.trim();

                match wallet_manager.export_wallet(name) {
                    Ok(mnemonic) => {
                        println!("📝 Recovery Phrase (KEEP SECRET):");
                        println!("{}", mnemonic.join(" "));
                    }
                    Err(e) => println!("❌ Error: {}", e),
                }
            }

            // Transaction commands
            "send" => {
                print!("From wallet: ");
                io::stdout().flush().unwrap();
                let mut from_wallet = String::new();
                io::stdin().read_line(&mut from_wallet).unwrap();
                let from_wallet = from_wallet.trim();

                if let Some(wallet) = wallet_manager.get_wallet(from_wallet) {
                    let from_addr = wallet.address.clone();

                    print!("To address: ");
                    io::stdout().flush().unwrap();
                    let mut to_addr = String::new();
                    io::stdin().read_line(&mut to_addr).unwrap();
                    let to_addr = to_addr.trim();

                    print!("Amount (ZOKA): ");
                    io::stdout().flush().unwrap();
                    let mut amount_str = String::new();
                    io::stdin().read_line(&mut amount_str).unwrap();

                    if let Ok(amount) = amount_str.trim().parse::<u64>() {
                        match transaction_pool.send_transaction(&from_addr, to_addr, amount, 1) {
                            Ok(tx_hash) => {
                                println!("✅ Transaction created!");
                                println!("📝 Hash: {}", tx_hash);
                                println!("⏳ Status: Pending");

                                if let Some(wallet) = wallet_manager.get_wallet_mut(from_wallet) {
                                    if wallet.balance >= amount {
                                        wallet.balance -= amount;
                                        println!("💰 New balance: {} ZOKA", wallet.balance);
                                    }
                                }
                            }
                            Err(e) => println!("❌ Error: {}", e),
                        }
                    } else {
                        println!("❌ Invalid amount");
                    }
                } else {
                    println!("❌ Wallet not found");
                }
            }

            "history" => {
                print!("Enter wallet name: ");
                io::stdout().flush().unwrap();
                let mut name = String::new();
                io::stdin().read_line(&mut name).unwrap();
                let name = name.trim();

                if let Some(wallet) = wallet_manager.get_wallet(name) {
                    if wallet.transactions.is_empty() {
                        println!("📭 No transactions yet");
                    } else {
                        println!("📋 Transaction History:");
                        for tx in &wallet.transactions {
                            let status_emoji = match tx.status {
                                TransactionStatus::Pending => "⏳",
                                TransactionStatus::Confirmed => "✅",
                                TransactionStatus::Failed => "❌",
                            };
                            println!(
                                "  {} {} → {} | {} ZOKA | {}",
                                status_emoji, tx.from, tx.to, tx.amount, tx.status
                            );
                        }
                    }
                } else {
                    println!("❌ Wallet not found");
                }
            }

            // Mining commands
            "mine status" => {
                let (blocks, rewards) = miner.get_stats();
                println!("⛏️  Mining Status:");
                println!("  Blocks Mined: {}", blocks);
                println!("  Total Rewards: {} ZOKA", rewards);
            }

            "mine start" => match miner.start_mining() {
                Ok(_) => println!("⛏️  Mining started..."),
                Err(e) => println!("❌ Error: {}", e),
            },

            "mine stop" => match miner.stop_mining() {
                Ok(_) => println!("⛏️  Mining stopped"),
                Err(e) => println!("❌ Error: {}", e),
            },

            // Pool commands
            "pool status" => {
                let (pending, confirmed, failed) = transaction_pool.get_stats();
                println!("🔄 Transaction Pool Status:");
                println!("  Pending: {}", pending);
                println!("  Confirmed: {}", confirmed);
                println!("  Failed: {}", failed);
            }

            "help" | "?" => {
                print_help();
            }

            "exit" | "quit" => {
                println!("👋 Goodbye!");
                break;
            }

            _ => {
                if !command.is_empty() {
                    println!("❌ Unknown command. Type 'help' for commands.");
                }
            }
        }
        println!();
    }
}

fn print_menu() {
    print!("\n");
}

fn print_help() {
    println!("📚 ZOKA Blockchain Commands:");
    println!();
    println!("💼 WALLET COMMANDS:");
    println!("  wallet create      - Create a new wallet");
    println!("  wallet import      - Import wallet from recovery phrase");
    println!("  wallet list        - List all wallets");
    println!("  wallet balance     - Check wallet balance");
    println!("  wallet export      - Export wallet recovery phrase");
    println!();
    println!("💸 TRANSACTION COMMANDS:");
    println!("  send               - Send ZOKA to another address");
    println!("  history            - View transaction history");
    println!();
    println!("⛏️  MINING COMMANDS:");
    println!("  mine status        - View mining statistics");
    println!("  mine start         - Start mining");
    println!("  mine stop          - Stop mining");
    println!();
    println!("🔄 POOL COMMANDS:");
    println!("  pool status        - Check transaction pool status");
    println!();
    println!("  help, ?            - Show this help");
    println!("  exit, quit         - Exit the CLI");
}
