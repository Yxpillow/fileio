const express = require('express');
const multer = require('multer');
const cors = require('cors');
const dotenv = require('dotenv');
const fs = require('fs');
const path = require('path');
const cluster = require('cluster');
const os = require('os');
const redis = require('redis');

// 导入集群配置
const clusterConfig = require('./cluster.config.js');

// 导入一致性哈希模块
const ConsistentHash = require('./consistent-hash.js');

// 加载环境变量
dotenv.config();

// 创建Express应用
const app = express();

// 配置CORS
app.use(cors());

// 设置JSON解析中间件
app.use(express.json());

// 从环境变量获取API密钥
const API_KEY = process.env.API_KEY;
// 对外主机名（用于写入Redis供跨节点跳转）
const PUBLIC_HOST = process.env.PUBLIC_HOST || 'localhost';

// 创建Redis客户端
const redisClient = redis.createClient({
  host: clusterConfig.redis.host,
  port: clusterConfig.redis.port,
  password: clusterConfig.redis.password
});

// 连接Redis
redisClient.connect().catch(err => {
  console.error('Redis连接失败:', err);
  // 如果Redis连接失败，继续运行但不使用集群功能
});

// 初始化一致性哈希环
const consistentHash = new ConsistentHash();

// 添加集群节点到一致性哈希环
clusterConfig.servers.forEach(server => {
  consistentHash.addNode(server);
});

// 认证中间件
const authenticateApiKey = (req, res, next) => {
  // 检查环境变量中是否配置了API密钥
  if (!API_KEY) {
    // 如果没有配置API密钥，允许所有请求（开发环境）
    return next();
  }
  
  // 从请求头获取API密钥
  const apiKey = req.headers['x-api-key'];
  
  if (!apiKey) {
    return res.status(401).json({ error: '未提供API密钥' });
  }
  
  // 验证API密钥
  if (apiKey !== API_KEY) {
    return res.status(403).json({ error: '无效的API密钥' });
  }
  
  next();
};

// 设置文件存储根目录
const ROOT_DIR = process.env.ROOT_DIR || './storage';

// 确保根目录存在
if (!fs.existsSync(ROOT_DIR)) {
  fs.mkdirSync(ROOT_DIR, { recursive: true });
}

// 配置Multer文件上传
const storage = multer.diskStorage({
  destination: function (req, file, cb) {
    // 从请求中获取bucket名称，默认为default
    const bucket = req.params.bucket || req.query.bucket || 'default';
    const bucketDir = path.join(ROOT_DIR, bucket);
    
    // 确保bucket目录存在
    if (!fs.existsSync(bucketDir)) {
      fs.mkdirSync(bucketDir, { recursive: true });
    }
    
    cb(null, bucketDir);
  },
  filename: function (req, file, cb) {
    // 使用时间戳和原始文件名确保唯一性
    const uniqueSuffix = Date.now() + '-' + Math.round(Math.random() * 1E9);
    const filename = uniqueSuffix + '-' + file.originalname;
    
    // 记录文件元数据到Redis（用于集群环境）
    const bucket = req.params.bucket || req.query.bucket || 'default';
    const fileKey = `${bucket}:${filename}`;
    
    // 获取当前服务器信息
    const currentServer = {
      id: `server-${process.pid}`,
      host: PUBLIC_HOST,
      port: process.env.PORT || 3001
    };
    
    // 存储文件位置信息到Redis
    redisClient.set(fileKey, JSON.stringify(currentServer)).catch(err => {
      console.error('Redis存储文件位置失败:', err);
    });
    
    cb(null, filename);
  }
});

const upload = multer({ storage: storage });

// API路由：列出所有储存桶
app.get('/api/buckets', authenticateApiKey, (req, res) => {
  fs.readdir(ROOT_DIR, (err, buckets) => {
    if (err) {
      return res.status(500).json({ error: '无法读取储存桶目录' });
    }

    // 获取每个储存桶的详细信息
    const bucketList = buckets.map(bucket => {
      const bucketPath = path.join(ROOT_DIR, bucket);
      const stats = fs.statSync(bucketPath);
      
      // 计算储存桶大小
      let size = 0;
      const files = fs.readdirSync(bucketPath);
      files.forEach(file => {
        const filePath = path.join(bucketPath, file);
        const fileStats = fs.statSync(filePath);
        size += fileStats.size;
      });
      
      return {
        name: bucket,
        size: size,
        created: stats.birthtime,
        modified: stats.mtime,
        fileCount: files.length
      };
    });

    res.json({ buckets: bucketList });
  });
});

// API路由：创建储存桶
app.post('/api/buckets', authenticateApiKey, (req, res) => {
  const { name } = req.body;
  
  if (!name) {
    return res.status(400).json({ error: '储存桶名称不能为空' });
  }
  
  // 验证储存桶名称格式
  if (!/^[a-z0-9][a-z0-9-]*[a-z0-9]$/.test(name)) {
    return res.status(400).json({ error: '储存桶名称只能包含小写字母、数字和连字符，且不能以连字符开头或结尾' });
  }
  
  const bucketDir = path.join(ROOT_DIR, name);
  
  // 检查储存桶是否已存在
  if (fs.existsSync(bucketDir)) {
    return res.status(409).json({ error: '储存桶已存在' });
  }
  
  // 创建储存桶目录
  try {
    fs.mkdirSync(bucketDir, { recursive: true });
    res.json({ success: true, bucket: { name } });
  } catch (err) {
    res.status(500).json({ error: '创建储存桶失败', details: err.message });
  }
});

// API路由：删除储存桶
app.delete('/api/buckets/:bucket', authenticateApiKey, (req, res) => {
  const bucket = req.params.bucket;
  const bucketDir = path.join(ROOT_DIR, bucket);
  
  // 检查储存桶是否存在
  if (!fs.existsSync(bucketDir)) {
    return res.status(404).json({ error: '储存桶不存在' });
  }
  
  // 删除储存桶及其内容
  try {
    fs.rmSync(bucketDir, { recursive: true, force: true });
    res.json({ success: true, message: '储存桶已成功删除' });
  } catch (err) {
    res.status(500).json({ error: '删除储存桶失败', details: err.message });
  }
});

// API路由：列出指定储存桶中的所有文件
app.get('/api/buckets/:bucket/files', authenticateApiKey, (req, res) => {
  const bucket = req.params.bucket;
  const bucketDir = path.join(ROOT_DIR, bucket);
  
  // 检查储存桶是否存在
  if (!fs.existsSync(bucketDir)) {
    return res.status(404).json({ error: '储存桶不存在' });
  }
  
  fs.readdir(bucketDir, (err, files) => {
    if (err) {
      return res.status(500).json({ error: '无法读取文件目录' });
    }

    // 获取每个文件的详细信息
    const fileList = files.map(file => {
      const filePath = path.join(bucketDir, file);
      const stats = fs.statSync(filePath);
      return {
        name: file,
        size: stats.size,
        created: stats.birthtime,
        modified: stats.mtime,
        bucket: bucket
      };
    });

    res.json({ files: fileList, bucket: bucket });
  });
});

// API路由：上传文件到指定储存桶
app.post('/api/buckets/:bucket/upload', authenticateApiKey, upload.single('file'), (req, res) => {
  if (!req.file) {
    return res.status(400).json({ error: '没有文件被上传' });
  }

  // 提取bucket名称
  const bucket = req.params.bucket;
  
  res.json({
    success: true,
    file: {
      name: req.file.filename,
      originalName: req.file.originalname,
      size: req.file.size,
      path: req.file.path,
      bucket: bucket
    }
  });
});

// API路由：下载指定储存桶中的文件
app.get('/api/buckets/:bucket/files/:filename', authenticateApiKey, async (req, res) => {
  const { bucket, filename } = req.params;
  const fileKey = `${bucket}:${filename}`;
  const filePath = path.join(ROOT_DIR, bucket, filename);

  try {
    // 检查文件是否存在于当前服务器
    if (fs.existsSync(filePath)) {
      // 设置响应头并发送文件（使用绝对路径）
      res.setHeader('Content-Disposition', `attachment; filename="${filename}"`);
      res.sendFile(path.resolve(filePath));
    } else if (redisClient.isOpen) {
      // 如果文件不存在于当前服务器，从Redis获取文件位置
      const fileLocation = await redisClient.get(fileKey);
      
      if (fileLocation) {
        const { host, port } = JSON.parse(fileLocation);
        // 重定向到文件所在的服务器
        res.redirect(`http://${host}:${port}/api/buckets/${bucket}/files/${filename}`);
      } else {
        // 文件不存在于任何服务器
        res.status(404).json({ error: '文件不存在' });
      }
    } else {
      // Redis未连接，仅检查当前服务器
      res.status(404).json({ error: '文件不存在' });
    }
  } catch (error) {
    console.error('下载文件时出错:', error);
    res.status(500).json({ error: '服务器内部错误' });
  }
});

// API路由：删除指定储存桶中的文件
app.delete('/api/buckets/:bucket/files/:filename', authenticateApiKey, async (req, res) => {
  const { bucket, filename } = req.params;
  const fileKey = `${bucket}:${filename}`;
  const filePath = path.join(ROOT_DIR, bucket, filename);

  try {
    // 从Redis获取文件位置
    if (redisClient.isOpen) {
      const fileLocation = await redisClient.get(fileKey);
      
      if (fileLocation) {
        const serverInfo = JSON.parse(fileLocation);
        const currentServer = `${serverInfo.host}:${serverInfo.port}`;
        const thisServer = `${PUBLIC_HOST}:${process.env.PORT || 3001}`;
        
        if (currentServer === thisServer) {
          // 文件在当前服务器，执行删除操作
          if (fs.existsSync(filePath)) {
            fs.unlinkSync(filePath);
            // 从Redis中删除文件位置信息
            await redisClient.del(fileKey);
            res.status(200).json({ message: '文件删除成功' });
          } else {
            res.status(404).json({ error: '文件不存在' });
          }
        } else {
          // 文件在其他服务器，返回错误（可以扩展为转发请求）
          res.status(404).json({ error: '文件不存在于当前服务器' });
        }
      } else {
        res.status(404).json({ error: '文件不存在' });
      }
    } else {
      // Redis未连接，仅在当前服务器执行操作
      if (fs.existsSync(filePath)) {
        fs.unlinkSync(filePath);
        res.status(200).json({ message: '文件删除成功' });
      } else {
        res.status(404).json({ error: '文件不存在' });
      }
    }
  } catch (error) {
    console.error('删除文件时出错:', error);
    res.status(500).json({ error: '文件删除失败: ' + error.message });
  }
});

// API路由：获取指定储存桶中文件的信息
app.get('/api/buckets/:bucket/files/:filename/info', authenticateApiKey, async (req, res) => {
  const { bucket, filename } = req.params;
  const fileKey = `${bucket}:${filename}`;
  const filePath = path.join(ROOT_DIR, bucket, filename);

  try {
    // 从Redis获取文件位置
    if (redisClient.isOpen) {
      const fileLocation = await redisClient.get(fileKey);
      
      if (fileLocation) {
        const serverInfo = JSON.parse(fileLocation);
        const currentServer = `${serverInfo.host}:${serverInfo.port}`;
        const thisServer = `${PUBLIC_HOST}:${process.env.PORT || 3001}`;
        
        if (currentServer === thisServer) {
          // 文件在当前服务器，获取文件信息
          if (fs.existsSync(filePath)) {
            const stats = fs.statSync(filePath);
            res.status(200).json({
              filename: filename,
              size: stats.size,
              createdAt: stats.birthtime,
              modifiedAt: stats.mtime,
              bucket: bucket,
              location: serverInfo
            });
          } else {
            res.status(404).json({ error: '文件不存在' });
          }
        } else {
          // 文件在其他服务器，返回错误（可以扩展为转发请求）
          res.status(404).json({ error: '文件不存在于当前服务器' });
        }
      } else {
        res.status(404).json({ error: '文件不存在' });
      }
    } else {
      // Redis未连接，仅在当前服务器获取信息
      if (fs.existsSync(filePath)) {
        const stats = fs.statSync(filePath);
        res.status(200).json({
          filename: filename,
          size: stats.size,
          createdAt: stats.birthtime,
          modifiedAt: stats.mtime,
          bucket: bucket
        });
      } else {
        res.status(404).json({ error: '文件不存在' });
      }
    }
  } catch (error) {
    console.error('获取文件信息时出错:', error);
    res.status(500).json({ error: '获取文件信息失败: ' + error.message });
  }
});

// API密钥相关路由已移除，改用简单的API密钥认证方式

// 静态文件服务（用于访问存储的文件）
app.use('/storage', authenticateApiKey, express.static(ROOT_DIR));

// 健康检查路由
app.get('/health', (req, res) => {
  res.json({ status: 'ok', message: '文件管理系统正在运行' });
});

// 集群结构路由（公开）
app.get('/structure', async (req, res) => {
  const server = { id: `server-${process.pid}`, host: PUBLIC_HOST, port: PORT };
  let nodes = [];
  let redisInfo = { connected: false };
  try {
    if (redisClient.isOpen) {
      redisInfo.connected = true;
      const members = await redisClient.sMembers('nodes');
      nodes = members.map(s => { try { return JSON.parse(s); } catch { return null; } }).filter(Boolean);
    }
  } catch (e) {}
  res.json({ server, nodes, redis: redisInfo, config: { servers: clusterConfig.servers } });
});

// 配置服务器端口
const PORT = process.env.PORT || 3001;

// 集群模式处理
if (clusterConfig.mode === 'cluster' && cluster.isPrimary) {
  // 获取CPU核心数
  const numCPUs = os.cpus().length;
  
  console.log(`主进程 ${process.pid} 正在运行`);
  console.log(`创建 ${numCPUs} 个工作进程...`);
  
  // 创建工作进程
  for (let i = 0; i < numCPUs; i++) {
    cluster.fork();
  }
  
  // 监听工作进程退出事件
  cluster.on('exit', (worker, code, signal) => {
    console.log(`工作进程 ${worker.process.pid} 已退出`);
    console.log('创建新的工作进程...');
    cluster.fork();
  });
} else {
  // 工作进程或单服务器模式
  app.listen(PORT, () => {
    console.log(`文件管理系统已启动，运行在 http://localhost:${PORT}`);
    console.log(`文件存储根目录: ${path.resolve(ROOT_DIR)}`);
    console.log(`进程ID: ${process.pid}`);
  });
}

// 节点注册与查询
app.post('/api/nodes/register', authenticateApiKey, async (req, res) => {
  const info = { id: `server-${process.pid}`, host: PUBLIC_HOST, port: PORT };
  try {
    if (redisClient.isOpen) {
      await redisClient.sAdd('nodes', JSON.stringify(info));
    }
    res.json({ success: true });
  } catch (e) {
    res.status(500).json({ error: '节点注册失败', details: e.message });
  }
});

app.get('/api/nodes', authenticateApiKey, async (req, res) => {
  try {
    if (!redisClient.isOpen) return res.json({ nodes: [] });
    const members = await redisClient.sMembers('nodes');
    const nodes = members.map(s => { try { return JSON.parse(s); } catch { return null; } }).filter(Boolean);
    res.json({ nodes });
  } catch (e) {
    res.status(500).json({ error: '获取节点失败', details: e.message });
  }
});
