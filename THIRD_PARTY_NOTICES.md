# WebRTC runtime 第三方组件

自有桥接代码在 `native/webrtc/`。WebRTC runtime 固定构建自未修改的上游 libdatachannel v0.24.4，提交 `8e330edebfe766b44f59be427e0454fd18964d4e`；固定子模块版本由该提交记录。构建脚本将各组件的完整许可证随 runtime 一起输出并打包。

| 组件 | 许可证 | 对应源码 |
| --- | --- | --- |
| libdatachannel | MPL-2.0 | [固定版本](https://github.com/paullouisageneau/libdatachannel/tree/8e330edebfe766b44f59be427e0454fd18964d4e) |
| libjuice | MPL-2.0 | [固定子模块](https://github.com/paullouisageneau/libjuice/tree/22145ec3473c73d811417ac9a6aee0de530aaa3f) |
| libsrtp | BSD-3-Clause | [固定子模块](https://github.com/cisco/libsrtp/tree/24b3bf8f19b6f5ab4cd2bcceb4f4064efca86fd5) |
| usrsctp | BSD-3-Clause | [固定子模块](https://github.com/sctplab/usrsctp/tree/fec583d54493f879d2ae44a743423bf8a04371ab) |
| plog | MIT | [固定子模块](https://github.com/SergiusTheBest/plog/tree/94899e0b926ac1b0f4750bfbd495167b4a6ae9ef) |
| OpenSSL 3 | Apache-2.0 | [官方源码](https://github.com/openssl/openssl)；实际版本由构建环境提供 |

本次 Mac runtime 使用 OpenSSL 3.6.3 静态链接；Ubuntu 使用系统 OpenSSL 3.5.5 动态库。WebSocket 和上游示例均不参与构建。usrsctp 属于上游编译依赖，产品没有以 DataChannel 传视频。

分发本 runtime 时保留随包的许可证、固定版本号和上述对应源码入口；如修改 MPL 覆盖的上游文件，需随分发提供相应文件的修改源码。项目没有引入 Xpra。

Mac 的许可证位于内嵌 WindowHost.app 的 `Contents/Resources/WebRTC/licenses/`；Linux 安装位置见 [WebRTC 指南](docs/webrtc.md)。这里记录新增 WebRTC 组件；系统 GStreamer、OpenH264、VideoToolbox 等沿用各自上游许可及系统分发方式。
