# Client for zk-shielded-pool contracts



## Tets endpoints

- /health
```
curl -i localhost:3000/health
```

- /deposit
```
curl -i -X POST localhost:3000/deposit \
  -H 'content-type: application/json' \
  -d '{
    "chunks": [100, 200, 300],
    "withdraw_addresses": [
      "Addr111111111111111111111111111111111111111",
      "Addr222222222222222222222222222222222222222",
      "Addr333333333333333333333333333333333333333"
    ],
    "wallet_address": "Wallet1111111111111111111111111111111111111"
  }'
```