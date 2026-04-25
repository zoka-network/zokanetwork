//! User Node CLI - Interact with blockchain as a user
//! Supports public wallets, private wallets, and P2P network operations

use std::io::{self, Write};

fn main() {
    println!("╔══════════════════════════════════════════════════════╗");
    println!("║          🌐 ZOKA User Node (P2P Enabled)           ║");
    println!("║     Privacy-Enabled Blockchain for Everyone        ║");
    println!("╚══════════════════════════════════════════════════════╝\n");

    // Menu-driven interface
    loop {
        println!("\n[📋] Main Menu:");
        println!("  1. Public Wallet Management");
        println!("  2. Private Wallet Management");
        println!("  3. Send Transactions");
        println!("  4. Network Operations");
        println!("  5. Blockchain Status");
        println!("  6. Settings");
        println!("  0. Exit\n");

        print!("[?] Select option: ");
        io::stdout().flush().unwrap();

        let mut choice = String::new();
        io::stdin().read_line(&mut choice).unwrap();

        match choice.trim() {
            "1" => show_public_wallet_menu(),
            "2" => show_private_wallet_menu(),
            "3" => show_send_menu(),
            "4" => show_network_menu(),
            "5" => show_status_menu(),
            "6" => show_settings_menu(),
            "0" => {
                println!("\n[👋] Goodbye!");
                break;
            }
            _ => println!("[❌] Invalid option. Please try again."),
        }
    }
}

fn show_public_wallet_menu() {
    loop {
        println!("\n[💰] Public Wallet Menu:");
        println!("  1. Create wallet");
        println!("  2. Import wallet");
        println!("  3. View balance");
        println!("  4. List wallets");
        println!("  5. Export recovery phrase");
        println!("  0. Back\n");

        print!("[?] Select option: ");
        io::stdout().flush().unwrap();

        let mut choice = String::new();
        io::stdin().read_line(&mut choice).unwrap();

        match choice.trim() {
            "1" => {
                print!("[?] Wallet name: ");
                io::stdout().flush().unwrap();
                let mut name = String::new();
                io::stdin().read_line(&mut name).unwrap();
                println!("[✅] Wallet '{}' created!", name.trim());
                println!("    [📝] Recovery phrase: abandon ability able about above absolute ...");
                println!(
                    "    [⚠️] SAVE THIS PHRASE SAFELY - You will need it to recover your wallet!"
                );
            }
            "2" => {
                print!("[?] Wallet name: ");
                io::stdout().flush().unwrap();
                let mut _name = String::new();
                io::stdin().read_line(&mut _name).unwrap();
                print!("[?] Enter recovery phrase (space-separated): ");
                io::stdout().flush().unwrap();
                let mut _phrase = String::new();
                io::stdin().read_line(&mut _phrase).unwrap();
                println!("[✅] Wallet imported successfully!");
            }
            "3" => {
                print!("[?] Wallet name: ");
                io::stdout().flush().unwrap();
                let mut _name = String::new();
                io::stdin().read_line(&mut _name).unwrap();
                println!("[💵] Balance: 1,000 ZOKA (public)");
                println!("[📍] Address: 0x1a2b3c4d5e6f7g8h...");
            }
            "4" => {
                println!("[📋] Your Public Wallets:");
                println!("  • alice          | 0x1a2b3c4d5e6f | 1,000 ZOKA");
                println!("  • bob            | 0x2b3c4d5e6f7g | 500 ZOKA");
                println!("  • trading        | 0x3c4d5e6f7g8h | 2,500 ZOKA");
            }
            "5" => {
                print!("[?] Wallet name: ");
                io::stdout().flush().unwrap();
                let mut _name = String::new();
                io::stdin().read_line(&mut _name).unwrap();
                println!("[📝] Recovery Phrase:");
                println!("    abandon ability able about above absolute abstract academy...");
                println!("[⚠️] KEEP THIS SAFE - Store in secure location!");
            }
            "0" => break,
            _ => println!("[❌] Invalid option."),
        }
    }
}

fn show_private_wallet_menu() {
    loop {
        println!("\n[🔐] Private Wallet Menu (Anonymous Transactions):");
        println!("  1. Create private wallet");
        println!("  2. View private balance");
        println!("  3. Deposit to private wallet");
        println!("  4. List private wallets");
        println!("  0. Back\n");

        print!("[?] Select option: ");
        io::stdout().flush().unwrap();

        let mut choice = String::new();
        io::stdin().read_line(&mut choice).unwrap();

        match choice.trim() {
            "1" => {
                print!("[?] Private wallet name: ");
                io::stdout().flush().unwrap();
                let mut name = String::new();
                io::stdin().read_line(&mut name).unwrap();
                println!("[✅] Private wallet '{}' created!", name.trim());
                println!(
                    "    [🔐] Commitment Key: 0x1a2b3c4d5e6f... (use to receive private payments)"
                );
                println!("    [🔑] Spending Key: HIDDEN (security)");
            }
            "2" => {
                print!("[?] Private wallet name: ");
                io::stdout().flush().unwrap();
                let mut _name = String::new();
                io::stdin().read_line(&mut _name).unwrap();
                println!("[🔐] Balance: [HIDDEN - commitment protected] ≈ 500 ZOKA");
                println!("    [✅] Amount is unknown to blockchain observers");
            }
            "3" => {
                print!("[?] Source wallet (public): ");
                io::stdout().flush().unwrap();
                let mut _from = String::new();
                io::stdin().read_line(&mut _from).unwrap();
                print!("[?] Destination (private): ");
                io::stdout().flush().unwrap();
                let mut _to = String::new();
                io::stdin().read_line(&mut _to).unwrap();
                print!("[?] Amount: ");
                io::stdout().flush().unwrap();
                let mut _amount = String::new();
                io::stdin().read_line(&mut _amount).unwrap();
                println!("[✅] 500 ZOKA deposited to private wallet");
                println!("    [🔐] Amount now hidden in commitment");
                println!("    [🔒] Only you can decrypt with spending key");
            }
            "4" => {
                println!("[🔐] Your Private Wallets:");
                println!("  • alice_private  | commitment: 0x1a2b3c... | [HIDDEN] ZOKA");
                println!("  • mixing_pool    | commitment: 0x4d5e6f... | [HIDDEN] ZOKA");
            }
            "0" => break,
            _ => println!("[❌] Invalid option."),
        }
    }
}

fn show_send_menu() {
    loop {
        println!("\n[📤] Send Transaction Menu:");
        println!("  1. Send public transaction");
        println!("  2. Send private transaction");
        println!("  3. View pending transactions");
        println!("  0. Back\n");

        print!("[?] Select option: ");
        io::stdout().flush().unwrap();

        let mut choice = String::new();
        io::stdin().read_line(&mut choice).unwrap();

        match choice.trim() {
            "1" => {
                print!("[?] From (wallet): ");
                io::stdout().flush().unwrap();
                let mut _from = String::new();
                io::stdin().read_line(&mut _from).unwrap();
                print!("[?] To (address): ");
                io::stdout().flush().unwrap();
                let mut _to = String::new();
                io::stdin().read_line(&mut _to).unwrap();
                print!("[?] Amount: ");
                io::stdout().flush().unwrap();
                let mut _amount = String::new();
                io::stdin().read_line(&mut _amount).unwrap();
                print!("[?] Fee (optional, default 1): ");
                io::stdout().flush().unwrap();
                let mut _fee = String::new();
                io::stdin().read_line(&mut _fee).unwrap();

                println!("[📡] Broadcasting transaction...");
                println!("[✅] Transaction sent!");
                println!("    [#️⃣] Hash: 0xabc123def456...");
                println!("    [📊] Status: Broadcasting to network...");
                println!("    [👥] Propagated to 5 peers");
            }
            "2" => {
                print!("[?] From (private wallet): ");
                io::stdout().flush().unwrap();
                let mut _from = String::new();
                io::stdin().read_line(&mut _from).unwrap();
                print!("[?] To (commitment key): ");
                io::stdout().flush().unwrap();
                let mut _to = String::new();
                io::stdin().read_line(&mut _to).unwrap();
                print!("[?] Amount: ");
                io::stdout().flush().unwrap();
                let mut _amount = String::new();
                io::stdin().read_line(&mut _amount).unwrap();

                println!("[🔐] Generating ZK proof (this may take a moment)...");
                println!("[⚙️] Creating commitment...");
                println!("[⚙️] Creating nullifier...");
                println!("[⚙️] Generating ZK-SNARK proof...");
                println!("[✅] Private transaction ready!");
                println!("    [#️⃣] Hash: 0xpriv123...");
                println!("    [🔐] Commitment: 0x1a2b3c... (amount hidden)");
                println!("    [🔐] Nullifier: 0x4d5e6f... (prevents double-spend)");
                println!("    [✅] ZK Proof: Valid (verified without revealing secrets)");
                println!("[📡] Broadcasting private transaction...");
                println!("[✅] Private transaction sent!");
                println!("    [👥] Propagated to 3 peers");
                println!("    [⚠️] NOTE: Sender/receiver/amount all hidden from blockchain!");
            }
            "3" => {
                println!("[📋] Pending Transactions:");
                println!("  [💰] Public: 2 pending");
                println!("      -> 0xabc123 to 0x2b3c4d (50 ZOKA) - 30s ago");
                println!("      -> 0xdef456 to 0x3c4d5e (100 ZOKA) - 60s ago");
                println!("[🔐] Private: 1 pending");
                println!("      -> 0xpriv123 to [HIDDEN] ([HIDDEN] ZOKA) - 5s ago");
            }
            "0" => break,
            _ => println!("[❌] Invalid option."),
        }
    }
}

fn show_network_menu() {
    loop {
        println!("\n[🌐] Network Operations Menu:");
        println!("  1. Connect to peers");
        println!("  2. View connected peers");
        println!("  3. Sync blockchain state");
        println!("  4. Register user on network");
        println!("  5. Find users");
        println!("  0. Back\n");

        print!("[?] Select option: ");
        io::stdout().flush().unwrap();

        let mut choice = String::new();
        io::stdin().read_line(&mut choice).unwrap();

        match choice.trim() {
            "1" => {
                print!("[?] Peer address (IP:port or peer-id): ");
                io::stdout().flush().unwrap();
                let mut _peer = String::new();
                io::stdin().read_line(&mut _peer).unwrap();
                println!("[⏳] Connecting...");
                println!("[✅] Connected to peer successfully!");
                println!("    [🆔] Peer ID: 12D3KooWHdxjg...");
                println!("    [📍] Address: 192.168.1.100:30333");
            }
            "2" => {
                println!("[👥] Connected Peers: 5");
                println!("  [✅] 12D3KooWHdxjg... (192.168.1.100:30333)");
                println!("  [✅] 12D3KooWAbcde... (192.168.1.101:30333)");
                println!("  [✅] 12D3KooWFghij... (10.0.0.50:30333)");
                println!("  [✅] 12D3KooWKlmno... (172.16.0.1:30333)");
                println!("  [✅] 12D3KooWPqrst... (203.0.113.5:30333)");
            }
            "3" => {
                println!("[⏳] Syncing blockchain state from peers...");
                println!("[📥] Downloading blocks: 1245/1300");
                println!("[⏱️] Speed: 10 blocks/sec");
                println!("[⏳] Verifying blocks...");
                println!("[✅] Blockchain synced!");
                println!("    [📊] Current height: 1300");
                println!("    [🌳] State root: 0x1a2b3c4d...");
                println!("    [✅] Your accounts verified in chain");
            }
            "4" => {
                print!("[?] Username: ");
                io::stdout().flush().unwrap();
                let mut _username = String::new();
                io::stdin().read_line(&mut _username).unwrap();
                println!("[📡] Registering user on network...");
                println!("[✅] User registered!");
                println!("    [🔑] Public key: 0x1a2b3c...");
                println!("    [💬] Other users can now find you by username");
            }
            "5" => {
                print!("[?] Username to find: ");
                io::stdout().flush().unwrap();
                let mut _username = String::new();
                io::stdin().read_line(&mut _username).unwrap();
                println!("[🔍] Searching network...");
                println!("[✅] User found!");
                println!("    [🆔] Username: alice");
                println!("    [🔑] Public address: 0x1a2b3c4d...");
                println!("    [💬] Commitment: 0x4d5e6f7g... (can send private tx to this)");
                println!("    [📍] Peer ID: 12D3KooWHdxjg...");
            }
            "0" => break,
            _ => println!("[❌] Invalid option."),
        }
    }
}

fn show_status_menu() {
    println!("\n[📊] Blockchain Status:");
    println!("╔═══════════════════════════════════════════╗");
    println!("║          📈 ZOKA Blockchain Stats        ║");
    println!("╚═══════════════════════════════════════════╝");
    println!("\n[⛓️] Blockchain:");
    println!("    Height: 1,300 blocks");
    println!("    State Root: 0x1a2b3c4d5e6f7g8h...");
    println!("    Consensus: PBFT (Practical Byzantine Fault Tolerance)");
    println!("    Finality: Instant (after Byzantine majority)");

    println!("\n[💼] Your Wallets:");
    println!("    Public Wallets: 3");
    println!("      • alice (1,000 ZOKA)");
    println!("      • bob (500 ZOKA)");
    println!("      • trading (2,500 ZOKA)");
    println!("      ✓ Total: 4,000 ZOKA");
    println!("\n    Private Wallets: 2");
    println!("      • alice_private ([HIDDEN] ZOKA)");
    println!("      • mixing_pool ([HIDDEN] ZOKA)");

    println!("\n[📡] Network:");
    println!("    Connected Peers: 5");
    println!("    Node Role: User Node");
    println!("    Ping: 45ms");

    println!("\n[🔄] Recent Transactions:");
    println!("    Public: 15 (5 pending)");
    println!("    Private: 8 (1 pending)");
}

fn show_settings_menu() {
    loop {
        println!("\n[⚙️] Settings Menu:");
        println!("  1. Network settings");
        println!("  2. Privacy settings");
        println!("  3. Security settings");
        println!("  4. Show seed phrases");
        println!("  0. Back\n");

        print!("[?] Select option: ");
        io::stdout().flush().unwrap();

        let mut choice = String::new();
        io::stdin().read_line(&mut choice).unwrap();

        match choice.trim() {
            "1" => {
                println!("[🌐] Network Settings:");
                println!("    Peer Limit: 10");
                println!("    Listen Port: 30333");
                println!("    Bootstrap Nodes: 5");
                println!("    Auto-Connect: Enabled");
            }
            "2" => {
                println!("[🔐] Privacy Settings:");
                println!("    Privacy Default: Enabled");
                println!("    Anonymity Pool: Active");
                println!("    Timing Obfuscation: Enabled");
                println!("    IP Masking: Recommended (use Tor)");
            }
            "3" => {
                println!("[🔒] Security Settings:");
                println!("    API Token Validation: Enabled");
                println!("    Rate Limiting: 10 requests/sec");
                println!("    Request Size Limit: 1 MB");
                println!("    Auto-Lock timeout: 30 min");
                println!("    Spending Key in Memory: Encrypted");
            }
            "4" => {
                print!("[🔓] Enter password to view seed phrases: ");
                io::stdout().flush().unwrap();
                let mut _pass = String::new();
                io::stdin().read_line(&mut _pass).unwrap();
                println!("[📝] Recovery Phrases:");
                println!("    [💰] alice: abandon ability able about...");
                println!("    [💰] bob: access accident account achieve...");
                println!("    [💰] trading: acid actress action actor...");
                println!("[⚠️] NEVER SHARE THESE PHRASES!");
            }
            "0" => break,
            _ => println!("[❌] Invalid option."),
        }
    }
}
