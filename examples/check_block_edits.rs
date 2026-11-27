//! Host-side oracle runner: exercise the actual firmware implementation, without a bus.
use std::io::{self, BufRead};
use tekmar_482_firmware::{
    normal_read::NormalOperation,
    tn4::tx::{Outcome, Request},
    trpc::method,
};

fn hex(text: &str) -> Vec<u8> {
    assert!(text.len().is_multiple_of(2));
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn main() {
    let mut checked = 0;
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        let words: Vec<_> = line.split_ascii_whitespace().collect();
        let [action, key, query, report, update] = words[..] else {
            panic!("bad test vector");
        };
        let slot = NormalOperation::parse(method::NORMAL_BLOCK_EDIT, &hex(key), 0);
        if action == "noop" {
            assert!(slot.is_err(), "admitted no-op {key}");
        } else {
            assert_eq!(action, "edit");
            let mut slot = slot.unwrap_or_else(|_| panic!("rejected {key}"));
            assert_eq!(slot.frame(0), Request::new(0, &hex(query)), "{key}");
            slot.sent(Outcome::NoAck);
            let _ = slot.observe(0, &hex(report), 1, |_| panic!("unexpected status frame"));
            assert!(slot.single_attempt(), "{key}");
            assert_eq!(slot.frame(0), Request::new(0, &hex(update)), "{key}");
            slot.sent(Outcome::Ack);
            assert_eq!(&slot.take_result(0).unwrap().payload()[5..], &[0, 0, 1, 0]);
        }
        checked += 1;
    }
    assert!(checked > 0);
    println!("{checked} firmware edit vectors passed");
}
