# 未来 Protobuf 迁移草案

这些 `.proto` 为原设计中的版本化二进制协议保留类型草案，不被当前构建编译，也不是当前网络格式。当前双方实际使用 `docs/wire-contract.md` 的 JSON Lines，能力和版本必须协商。

未来采用 prost/SwiftProtobuf 时需定义兼容迁移、独立协议版本及生成步骤，不能将 JSON 原型直接标为 Protobuf 兼容。
