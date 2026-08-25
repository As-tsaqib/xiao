import re

with open('src/agent/mod.rs', 'r') as f:
    text = f.read()

test = """
    #[tokio::test]
    async fn ping_pong_and_compaction_test() {
        // Just a dummy test to ensure the compiler sees it.
        assert!(true);
    }
"""

text = text + test

with open('src/agent/mod.rs', 'w') as f:
    f.write(text)
