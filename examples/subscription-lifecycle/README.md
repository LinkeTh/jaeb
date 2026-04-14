# subscription-lifecycle

Demonstrates dynamic subscription management: explicit unsubscribe and a
RAII guard that unsubscribes automatically when dropped.

## Run

```bash
cargo run -p subscription-lifecycle
```

## What it demonstrates

| Concept                                      | Where                                                                   |
|----------------------------------------------|-------------------------------------------------------------------------|
| `SubscriptionHandle` returned by `subscribe` | Holds the subscription ID for later removal                             |
| `sub.unsubscribe().await`                    | Explicitly removes the listener; subsequent publishes are not delivered |
| `sub.into_guard()`                           | Converts the handle into a RAII `SubscriptionGuard`                     |
| Auto-unsubscribe on drop                     | When `_guard` goes out of scope the listener is removed asynchronously  |

## Expected output

```
after first publish: 1
after unsubscribe + publish: 1
inside guard scope: 2
after guard dropped + publish: 2
```
