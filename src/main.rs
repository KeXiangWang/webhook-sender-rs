use clap::{Arg, Command};
use hmac::{Hmac, Mac};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use std::{
    fs,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

const WEBHOOK_SERVER_URL: &str = "http://127.0.0.1:4560/webhook/dev/public/github_hmac_sha1";

//
// ============== Data Structures & Constants ==============
//

/// Holds the parsed command-line arguments
#[derive(Debug)]
struct AppArgs {
    secret: String,
    total_requests: usize,
    concurrency: usize,
    duration_secs: u64,
}

/// The JSON payload structure to be sent to the webhook
#[derive(Serialize, Deserialize, Clone)]
struct WebhookMessage {
    id: String,
    event: String,
    source: String,
    auth_algo: String,
    data: MessageData,
    timestamp: u64,
}

/// Embedded data structure
#[derive(Serialize, Deserialize, Clone)]
struct MessageData {
    order_id: u32,
    customer_name: String,
    amount: f64,
    currency: String,
}

/// Global counters shared across threads
/// Wrapped in a struct for unified management
#[derive(Default)]
struct Counters {
    completed_requests: AtomicUsize,
    interval_requests: AtomicUsize,
    total_latency_ns: AtomicU64,
}

// Interval in seconds for periodic reporting
static REPORT_INTERVAL: u64 = 5;
// Flush local counters to global counters after this many requests
static FLUSH_INTERVAL: usize = 100;

//
// ============== Main Entry Point ==============
//

fn main() {
    // Parse command-line arguments
    let args = parse_args();

    // Prepare a timestamped log file name
    let now_str = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let report_file = format!("report-{}.txt", now_str);
    println!("Report written to {report_file}");

    // Run the load test
    run_load_test(&args, &report_file);

    println!("Report written to {report_file}");
}

//
// ============== Functions ==============
//

/// Parse command-line arguments
fn parse_args() -> AppArgs {
    let matches = Command::new("Simulate sending Webhook messages (Rust, modularized)")
        .arg(
            Arg::new("secret")
                .long("secret")
                .default_value("TEST_WEBHOOK")
                .help("Secret key for generating signature"),
        )
        .arg(
            Arg::new("total_requests")
                .long("total-requests")
                .default_value("10000")
                .value_parser(clap::value_parser!(usize))
                .help("Total number of requests to send (default: 10000)"),
        )
        .arg(
            Arg::new("unlimited")
                .long("unlimited")
                .action(clap::ArgAction::SetTrue)
                .help("Send unlimited requests (overrides --total-requests)"),
        )
        .arg(
            Arg::new("concurrency")
                .long("concurrency")
                .default_value("10")
                .value_parser(clap::value_parser!(usize))
                .help("Number of concurrent threads (default: 10)"),
        )
        .arg(
            Arg::new("duration")
                .long("duration")
                .default_value("180")
                .value_parser(clap::value_parser!(u64))
                .help("Duration to run in seconds (default: 180)"),
        )
        .get_matches();

    let secret = matches.get_one::<String>("secret").unwrap().to_string();
    let mut total_requests = *matches.get_one::<usize>("total_requests").unwrap();
    let unlimited = matches.get_flag("unlimited");
    let concurrency = *matches.get_one::<usize>("concurrency").unwrap();
    let duration_secs = *matches.get_one::<u64>("duration").unwrap();

    if unlimited {
        // Use 0 to represent unlimited requests
        total_requests = 0;
    }

    AppArgs {
        secret,
        total_requests,
        concurrency,
        duration_secs,
    }
}

/// Controls the main flow of the load test
fn run_load_test(args: &AppArgs, report_file: &str) {
    // Create a global counters struct wrapped in Arc for multi-thread sharing
    let counters = Arc::new(Counters::default());

    // Start a thread that periodically reports throughput, latency, etc.
    {
        let counters_for_report = Arc::clone(&counters);
        let report_file = report_file.to_string();
        thread::spawn(move || {
            reporting_thread_loop(counters_for_report, report_file);
        });
    }

    // Start worker threads
    let start_time = Instant::now();
    let mut handles = Vec::with_capacity(args.concurrency);

    for _ in 0..args.concurrency {
        let secret_clone = args.secret.clone();
        let counters_clone = Arc::clone(&counters);
        let total_requests = args.total_requests; // a copy
        let duration_secs = args.duration_secs; // a copy
        let handle = thread::spawn(move || {
            worker_loop(
                &secret_clone,
                counters_clone,
                start_time,
                total_requests,
                duration_secs,
            );
        });
        handles.push(handle);
    }

    // Wait for all worker threads to finish
    for h in handles {
        let _ = h.join();
    }

    // Print final stats
    let final_completed = counters.completed_requests.load(Ordering::Relaxed);
    let final_total_ns = counters.total_latency_ns.load(Ordering::Relaxed);
    let final_avg_latency_s = if final_completed > 0 {
        (final_total_ns as f64 / 1e9) / final_completed as f64
    } else {
        0.0
    };
    let line = format!(
        "Final Stats: Requests Completed: {}, Average Latency: {:.6}s",
        final_completed, final_avg_latency_s
    );
    println!("{}", line);
    let _ = fs::write(report_file, format!("{}\n", line).as_bytes());
}

/// The reporting thread that periodically prints stats to console and file
fn reporting_thread_loop(counters: Arc<Counters>, report_file: String) {
    loop {
        thread::sleep(Duration::from_secs(REPORT_INTERVAL));

        // Get the number of requests for this interval
        let interval_count = counters.interval_requests.swap(0, Ordering::Relaxed);
        let cpl = counters.completed_requests.load(Ordering::Relaxed);
        let total_ns = counters.total_latency_ns.load(Ordering::Relaxed);

        // Compute throughput & average latency
        let throughput = interval_count as f64 / (REPORT_INTERVAL as f64);
        let avg_latency_s = if cpl > 0 {
            (total_ns as f64 / 1e9) / (cpl as f64)
        } else {
            0.0
        };

        // Print and append to file
        let line = format!(
            "Current Throughput: {:.2} RPS | Total Requests: {} | Average Latency: {:.6}s",
            throughput, cpl, avg_latency_s
        );
        println!("{}", line);
        let _ = fs::write(&report_file, format!("{}\n", line).as_bytes());
    }
}

/// The worker loop for each thread, sending requests until time or total requests is reached
fn worker_loop(
    secret: &str,
    counters: Arc<Counters>,
    start_time: Instant,
    total_requests: usize,
    duration_secs: u64,
) {
    let client = build_http_client();

    // Prepare a template message
    let base_message = WebhookMessage {
        id: "".to_string(),
        event: "order.created".to_string(),
        source: "placeholder".to_string(),
        auth_algo: "placeholder".to_string(),
        data: MessageData {
            order_id: 1234,
            customer_name: "Alice".to_string(),
            amount: 99.99,
            currency: "USD".to_string(),
        },
        timestamp: 1639581841,
    };

    // Local counters to reduce the overhead of atomic operations
    let mut completed_local = 0usize;
    let mut interval_local = 0usize;
    let mut total_latency_local = 0u64;

    loop {
        // Check if duration is up
        if start_time.elapsed().as_secs() >= duration_secs {
            break;
        }

        // If total_requests > 0, check if it is reached
        if total_requests > 0 {
            let global_count = counters.completed_requests.load(Ordering::Relaxed);
            if global_count + completed_local >= total_requests {
                break;
            }
        }

        // Build the request payload
        let mut msg = base_message.clone();
        msg.id = Uuid::new_v4().to_string();
        msg.source = "github".to_string();
        msg.auth_algo = "hmac_sha1".to_string();

        let payload_json = match serde_json::to_string(&msg) {
            Ok(s) => s,
            Err(_) => {
                eprintln!("Failed to serialize JSON");
                continue;
            }
        };

        // Generate the signature
        let signature = generate_github_hmac_sha1(secret, &payload_json);

        // Send the request
        let start_req_time = Instant::now();
        let resp = client
            .post(WEBHOOK_SERVER_URL)
            .header("Content-Type", "application/json")
            .header("X-Hub-Signature", &signature)
            .body(payload_json)
            .send();

        let latency_ns = start_req_time.elapsed().as_nanos() as u64;

        match resp {
            Ok(r) => {
                if r.status().is_success() {
                    completed_local += 1;
                    interval_local += 1;
                    total_latency_local += latency_ns;
                } else {
                    eprintln!("Request failed with status {}", r.status());
                }
            }
            Err(e) => {
                eprintln!("Request error: {}", e);
            }
        }

        // Periodically flush local counters
        if completed_local > 0 && (completed_local % FLUSH_INTERVAL == 0) {
            counters
                .completed_requests
                .fetch_add(completed_local, Ordering::Relaxed);
            counters
                .interval_requests
                .fetch_add(interval_local, Ordering::Relaxed);
            counters
                .total_latency_ns
                .fetch_add(total_latency_local, Ordering::Relaxed);

            completed_local = 0;
            interval_local = 0;
            total_latency_local = 0;
        }
    }

    // Flush any leftover counters when the thread finishes
    if completed_local > 0 {
        counters
            .completed_requests
            .fetch_add(completed_local, Ordering::Relaxed);
        counters
            .interval_requests
            .fetch_add(interval_local, Ordering::Relaxed);
        counters
            .total_latency_ns
            .fetch_add(total_latency_local, Ordering::Relaxed);
    }
}

/// Build a blocking HTTP client with 5s timeout
fn build_http_client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("Failed to build reqwest Client")
}

/// Generate a GitHub-style HMAC-SHA1 signature: "sha1=<hex>"
fn generate_github_hmac_sha1(secret: &str, payload: &str) -> String {
    type HmacSha1 = Hmac<Sha1>;
    let mut mac =
        HmacSha1::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
    mac.update(payload.as_bytes());
    let result = mac.finalize().into_bytes();
    format!("sha1={}", hex::encode(result))
}
