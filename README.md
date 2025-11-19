# 文件管理系统（双版本：Node + Rust）

一个文件管理系统，提供RESTful API用于文件上传、下载、删除和列出文件等功能。支持储存桶管理、API密钥认证、集群部署和分布式文件存储。

本项目同时提供两种实现：
- 版本 A（Node.js，Express）：位于项目根目录
- 版本 B（Rust，Axum）：位于 `rust-b/` 目录

## 功能特性

- 📁 **储存桶管理**：创建、列出、删除储存桶
- 📄 **文件操作**：上传、下载、删除、列出文件
- 📊 **文件信息**：获取文件详细信息（大小、创建时间、修改时间等）
- 🔑 **API密钥认证**：支持基于API密钥的访问控制
- 📡 **集群支持**：多服务器部署，支持负载均衡
- 📂 **分布式存储**：基于一致性哈希的文件分布策略
- 🔄 **Redis集成**：用于集群通信和状态管理
- 🌐 **CORS支持**：允许跨域访问
- 🔧 **环境配置**：支持通过.env文件灵活配置

## 安装

1. 克隆或下载项目

2. 选择实现并安装依赖：

Node 版本 A：
```bash
npm install
```

Rust 版本 B（需要已安装 Rust 工具链）：
```bash
cd rust-b
cargo build
```

3. 创建.env文件：
```bash
# 服务器配置
PORT=3001
ROOT_DIR=./storage

# API密钥配置
# 如果不设置API_KEY，系统将允许所有请求（开发环境）
API_KEY=your-api-secret-key

# 集群配置（可选）
CLUSTER_MODE=single
REDIS_HOST=localhost
REDIS_PORT=6379
REDIS_PASSWORD=

## 运行（Node 版本 A）

```bash
# 启动服务器
npm start

# 或使用dev命令
npm run dev
```

服务器将在 http://localhost:3001 启动

## 运行（Rust 版本 B）

Rust 版默认与 Node 版保持相同的路由语义（无鉴权默认开放）。

```bash
# 在 rust-b 目录下运行（可按需设置环境变量）
cd rust-b
PORT=4002 ROOT_DIR=./storage cargo run

# 示例：创建储存桶
curl -X POST http://localhost:4002/api/buckets -H 'Content-Type: application/json' -d '{"name":"default"}'

# 示例：上传文件
curl -F 'file=@./test-upload.txt' http://localhost:4002/api/buckets/default/upload

# 示例：列出文件
curl http://localhost:4002/api/buckets/default/files
```

## API文档

### 认证
所有API（除了健康检查）都需要在请求头中包含API密钥：
```
X-API-Key: your-api-secret-key
```

### 储存桶管理

#### 列出所有储存桶
- **方法**：GET
- **URL**：/api/buckets
- **响应**：
```json
{
  "buckets": [
    {
      "name": "test-bucket",
      "size": 1024,
      "created": "2023-05-10T10:00:00.000Z",
      "modified": "2023-05-10T10:30:00.000Z",
      "fileCount": 5
    }
  ]
}
```

#### 创建储存桶
- **方法**：POST
- **URL**：/api/buckets
- **请求体**：
```json
{
  "name": "new-bucket"
}
```
- **响应**：
```json
{
  "success": true,
  "bucket": {
    "name": "new-bucket"
  }
}
```

#### 删除储存桶
- **方法**：DELETE
- **URL**：/api/buckets/:bucket
- **响应**：
```json
{
  "success": true,
  "message": "储存桶已成功删除"
}
```

### 文件操作

#### 列出储存桶中的文件
- **方法**：GET
- **URL**：/api/buckets/:bucket/files
- **响应**：
```json
{
  "files": [
    {
      "name": "file1.txt",
      "size": 1024,
      "created": "2023-05-10T10:00:00.000Z",
      "modified": "2023-05-10T10:30:00.000Z",
      "bucket": "test-bucket"
    }
  ],
  "bucket": "test-bucket"
}
```

#### 上传文件到储存桶
- **方法**：POST
- **URL**：/api/buckets/:bucket/upload
- **表单字段**：file (文件)
- **响应**：
```json
{
  "success": true,
  "file": {
    "name": "timestamp-file.txt",
    "originalName": "file.txt",
    "size": 1024,
    "path": "./storage/test-bucket/timestamp-file.txt",
    "bucket": "test-bucket"
  }
}
```

#### 下载文件
- **方法**：GET
- **URL**：/api/buckets/:bucket/files/:filename
- **响应**：文件下载

#### 删除文件
- **方法**：DELETE
- **URL**：/api/buckets/:bucket/files/:filename
- **响应**：
```json
{
  "message": "文件删除成功"
}
```

#### 获取文件信息
- **方法**：GET
- **URL**：/api/buckets/:bucket/files/:filename/info
- **响应**：
```json
{
  "filename": "file.txt",
  "size": 1024,
  "createdAt": "2023-05-10T10:00:00.000Z",
  "modifiedAt": "2023-05-10T10:30:00.000Z",
  "bucket": "test-bucket",
  "location": {
    "id": "server-12345",
    "host": "localhost",
    "port": "3001"
  }
}
```

### 健康检查
- **方法**：GET
- **URL**：/health
- **响应**：
```json
{
  "status": "ok",
  "message": "文件管理系统正在运行"
}
```

## 静态文件访问

上传的文件可以通过以下URL直接访问（需要包含API密钥）：
```
http://localhost:3001/storage/:bucket/:filename
```

示例：
```
curl -H "X-API-Key: your-api-secret-key" http://localhost:3001/storage/test-bucket/file.txt
```

## 技术栈

**Node 版本 A**
- Node.js、Express.js、Multer、CORS、Dotenv
- Redis、node-cluster、一致性哈希算法（分布式文件存储）

**Rust 版本 B**
- Rust、Axum、Tokio、tower-http（CORS）、dotenvy
- 本地存储实现，接口与 Node 版一致；可扩展接入 Redis 与多节点

## 集群部署

### 配置文件

系统支持通过`cluster.config.js`文件配置集群参数：

- `mode`：集群模式（'single'或'cluster'）
- `servers`：服务器列表
- `loadBalancer`：负载均衡配置
- `sharedStorage`：共享存储配置
- `redis`：Redis配置

### 运行多个实例

1. 启动Redis服务器（如果使用集群模式）
2. 配置不同的PORT环境变量
3. 启动多个服务器实例

```bash
PORT=3001 node index.js
PORT=3002 node index.js
PORT=3003 node index.js
```

## 分布式存储

系统使用一致性哈希算法将文件分布到不同的服务器节点上。文件位置信息存储在Redis中，允许集群中的任何节点找到文件的实际位置。

## 许可证

ISC
