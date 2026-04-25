use std::io::{self, Write};
use zoka::mining::UserMiner;
use zoka::private_wallet::PrivateWalletManager;
use zoka::transaction_pool::TransactionPool;
/// Enhanced User CLI with Privacy Support
/// Supports both public and private transactions
use zoka::wallet::WalletManager;

fn main() {
    println!("\n🔐 ========================================");
    println!("   ZOKA Blockchain - Private Edition");
    println!("   Enhanced CLI with Privacy Features");
    println!("======================================== 🔐\n");

    let mut wallet_manager = WalletManager::new();
    let mut transaction_pool = TransactionPool::new();
    let mut private_wallet_manager = PrivateWalletManager::new();
    let miner = UserMiner::new();

    loop {
        print!("[?] Enter command: ");
        io::stdout().flush().unwrap();

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .expect("Failed to read input");

        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        let parts: Vec<&str> = input.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        match parts[0] {
            // === REGULAR WALLET COMMANDS ===
            "wallet" => {
                if parts.len() < 2 {
                    println!("[❌] Usage: wallet create|import|list|balance|export [name]");
                    continue;
                }

                match parts[1] {
                    "create" => {
                        let name = if parts.len() > 2 { parts[2] } else { "default" };
                        match wallet_manager.create_wallet(name) {
                            Ok((addr, mnemonic)) => {
                                println!("[✅] Wallet '{}' created!", name);
                                println!("    Address: {}", &addr[..std::cmp::min(32, addr.len())]);
                                println!("    Mnemonic: {} words", mnemonic.len());
                                println!("    [SAVE THIS]: {}", mnemonic.join(" "));
                            }
                            Err(e) => println!("[❌] Error: {}", e),
                        }
                    }
                    "list" => {
                        let wallets = wallet_manager.list_wallets();
                        if wallets.is_empty() {
                            println!("[⚠️ ] No wallets yet");
                        } else {
                            println!("[📋] Regular Wallets:");
                            for (name, addr, balance) in wallets {
                                println!("    {} | {} | {} ZOKA", name, &addr[..16], balance);
                            }
                        }
                    }
                    "balance" => {
                        let name = if parts.len() > 2 { parts[2] } else { "default" };
                        match wallet_manager.get_balance(name) {
                            Some(balance) => println!("[💰] {}: {} ZOKA", name, balance),
                            None => println!("[❌] Wallet not found"),
                        }
                    }
                    "export" => {
                        let name = if parts.len() > 2 { parts[2] } else { "default" };
                        match wallet_manager.export_wallet(name) {
                            Ok(mnemonic) => {
                                println!("[🔐] Recovery phrase for '{}':", name);
                                println!("    {}", mnemonic.join(" "));
                                println!("[⚠️ ] SAVE THIS PHRASE SECURELY!");
                            }
                            Err(e) => println!("[❌] Error: {}", e),
                        }
                    }
                    "import" => {
                        let name = if parts.len() > 2 {
                            parts[2]
                        } else {
                            "imported"
                        };
                        print!("[?] Paste recovery phrase: ");
                        io::stdout().flush().unwrap();
                        let mut phrase = String::new();
                        io::stdin().read_line(&mut phrase).unwrap();
                        let mnemonic_words: Vec<&str> = phrase.trim().split_whitespace().collect();
                        let mnemonic = mnemonic_words
                            .iter()
                            .map(|s| s.to_string())
                            .collect::<Vec<_>>();
                        match wallet_manager.import_wallet(name, &mnemonic) {
                            Ok(addr) => {
                                println!("[✅] Wallet imported!");
                                println!("    Address: {}", &addr[..std::cmp::min(32, addr.len())]);
                            }
                            Err(e) => println!("[❌] Error: {}", e),
                        }
                    }
                    _ => println!("[❌] Unknown wallet command"),
                }
            }

            // === PRIVATE WALLET COMMANDS ===
            "private" => {
                if parts.len() < 2 {
                    println!("[🔐] Usage: private create|list|balance|deposit [name] [amount]");
                    continue;
                }

                match parts[1] {
                    "create" => {
                        let name = if parts.len() > 2 {
                            parts[2]
                        } else {
                            "private_default"
                        };
                        match private_wallet_manager.create_private_wallet(name) {
                            Ok(commitment) => {
                                println!("[🔐] Private wallet '{}' created!", name);
                                println!("    Commitment: {}...", &commitment[..16]);
                                println!("    [✅] No amount parameters visible");
                            }
                            Err(e) => println!("[❌] Error: {}", e),
                        }
                    }
                    "list" => {
                        let wallets = private_wallet_manager.list_private_wallets();
                        if wallets.is_empty() {
                            println!("[⚠️ ] No private wallets yet");
                        } else {
                            println!("[🔐] Private Wallets:");
                            for (name, commitment, balance, tx_count) in wallets {
                                println!(
                                    "    {} | Commitment: {}... | Balance: {} (hidden) | {} txs",
                                    name,
                                    &commitment[..16],
                                    balance,
                                    tx_count
                                );
                            }
                        }
                    }
                    "balance" => {
                        let name = if parts.len() > 2 {
                            parts[2]
                        } else {
                            "private_default"
                        };
                        match private_wallet_manager.get_private_wallet(name) {
                            Some(wallet) => {
                                println!("[🔐] {}: {} (private)", name, wallet.private_balance);
                                println!("    [✅] Amount is commitment-hidden");
                            }
                            None => println!("[❌] Wallet not found"),
                        }
                    }
                    "deposit" => {
                        if parts.len() < 4 {
                            println!("[❌] Usage: private deposit [name] [amount]");
                            continue;
                        }
                        let name = parts[2];
                        let amount: u64 = parts[3].parse().unwrap_or(0);

                        match private_wallet_manager.deposit_to_private_wallet(name, amount) {
                            Ok(_) => {
                                println!(
                                    "[✅] {} ZOKA deposited to private wallet '{}'",
                                    amount, name
                                );
                                println!("    [🔐] Amount is now commitment-protected");
                            }
                            Err(e) => println!("[❌] Error: {}", e),
                        }
                    }
                    _ => println!("[❌] Unknown private command"),
                }
            }

            // === REGULAR SEND ===
            "send" => {
                if parts.len() < 4 {
                    println!("[❌] Usage: send [from_addr] [to_addr] [amount]");
                    continue;
                }

                let from = parts[1];
                let to = parts[2];
                let amount: u64 = parts[3].parse().unwrap_or(0);

                match transaction_pool.send_transaction(from, to, amount, 1) {
                    Ok(hash) => {
                        println!("[✅] Public transaction sent!");
                        println!("    Hash: {}...", &hash[..16]);
                        println!("    From: {}", &from[..16]);
                        println!("    To: {}", &to[..16]);
                        println!("    Amount: {} ZOKA (public)", amount);
                    }
                    Err(e) => println!("[❌] Error: {}", e),
                }
            }

            // === PRIVATE SEND (with ZK) ===
            "private-send" => {
                if parts.len() < 4 {
                    println!("[🔐] Usage: private-send [from_wallet] [to_commitment] [amount]");
                    continue;
                }

                let from_wallet = parts[1];
                let to_commitment = parts[2].to_string();
                let amount: u64 = parts[3].parse().unwrap_or(0);

                match private_wallet_manager.send_private_transaction(
                    from_wallet,
                    to_commitment.clone(),
                    amount,
                ) {
                    Ok(hash) => {
                        println!("[🔐] Private transaction sent!");
                        println!("    Hash: {}...", &hash[..16]);
                        println!("    From: {} (private)", from_wallet);
                        println!("    To: {}... (hidden)", &to_commitment[..16]);
                        println!("    Amount: [HIDDEN] (commitment-protected)");
                        println!("    Status: Generating ZK proof...");
                        let (pending, verified, _, _) =
                            private_wallet_manager.get_private_pool_stats();
                        println!("    Pool: {} pending, {} verified", pending, verified);
                    }
                    Err(e) => println!("[❌] Error: {}", e),
                }
            }

            // === HISTORY ===
            "history" => {
                if parts.len() < 2 {
                    println!("[❌] Usage: history [address|wallet]");
                    continue;
                }
                let addr = parts[1];
                let txs = transaction_pool.get_history(addr);
                println!("[📜] {} public transactions", txs.len());
                for (i, tx) in txs.iter().take(5).enumerate() {
                    println!(
                        "    {} | {} | Amount: {} | Status: {}",
                        i + 1,
                        &tx.to[..16],
                        tx.amount,
                        tx.status
                    );
                }
            }

            // === POOL STATUS ===
            "pool" => {
                if parts.len() > 1 && parts[1] == "status" {
                    let (pending, confirmed, failed) = transaction_pool.get_stats();
                    println!("[📊] Public Pool:");
                    println!("    Pending: {}", pending);
                    println!("    Confirmed: {}", confirmed);
                    println!("    Failed: {}", failed);

                    let (priv_pending, priv_verified, priv_confirmed, priv_failed) =
                        private_wallet_manager.get_private_pool_stats();
                    println!("[🔐] Private Pool:");
                    println!("    Pending: {}", priv_pending);
                    println!("    Verified: {}", priv_verified);
                    println!("    Confirmed: {}", priv_confirmed);
                    println!("    Failed: {}", priv_failed);
                } else {
                    println!("[❌] Usage: pool status");
                }
            }

            // === MINING ===
            "mine" => {
                if parts.len() < 2 {
                    println!("[❌] Usage: mine start|stop|status");
                    continue;
                }

                match parts[1] {
                    "start" => match miner.start_mining() {
                        Ok(_) => println!("[⛏️ ] Mining started..."),
                        Err(e) => println!("[❌] Error: {}", e),
                    },
                    "stop" => match miner.stop_mining() {
                        Ok(_) => println!("[⏹️ ] Mining stopped"),
                        Err(e) => println!("[❌] Error: {}", e),
                    },
                    "status" => {
                        let (blocks, rewards) = miner.get_stats();
                        println!("[⛏️ ] Mining Stats:");
                        println!("    Blocks mined: {}", blocks);
                        println!("    Total rewards: {} ZOKA", rewards);
                    }
                    _ => println!("[❌] Unknown mine command"),
                }
            }

            // === INFO & HELP ===
            "help" => {
                println!("\n[ℹ️ ] ZOKA CLI - Commands:");
                println!("\n[📋] Regular Transactions:");
                println!("  wallet create [name]         - Create public wallet");
                println!("  wallet list                  - Show all wallets");
                println!("  wallet balance [name]        - Check wallet balance");
                println!("  wallet export [name]         - Get recovery phrase");
                println!("  wallet import [name]         - Restore from phrase");
                println!("  send [from] [to] [amount]    - Send public transaction");
                println!("  history [address]            - View transaction history");
                println!("  pool status                  - Check transaction pool");

                println!("\n[🔐] Private Transactions (ZK-enabled):");
                println!("  private create [name]        - Create private wallet");
                println!("  private list                 - Show private wallets");
                println!("  private balance [name]       - Check private balance");
                println!("  private deposit [name] [amt] - Add funds to private wallet");
                println!("  private-send [wallet] [to] [amt] - Send private tx (ZK proof)");

                println!("\n[⛏️ ] Mining:");
                println!("  mine start                   - Start mining blocks");
                println!("  mine stop                    - Stop mining");
                println!("  mine status                  - View mining rewards");

                println!("\n[ℹ️ ] System:");
                println!("  help                         - Show this help");
                println!("  exit                         - Quit CLI\n");
            }

            "exit" => {
                println!("[✅] Goodbye!");
                break;
            }

            _ => println!("[❌] Unknown command. Try 'help'"),
        }

        println!();
    }
}
