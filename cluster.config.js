// 集群配置文件
module.exports = {
  // 集群模式：'single'（单服务器）或 'cluster'（多服务器）
  mode: process.env.CLUSTER_MODE || 'single',
  
  // 服务器列表（仅在cluster模式下使用）
  servers: [
    { id: 'server1', host: 'localhost', port: 3001 },
    { id: 'server2', host: 'localhost', port: 3002 },
    { id: 'server3', host: 'localhost', port: 3003 }
  ],
  
  // 负载均衡配置
  loadBalancer: {
    // 负载均衡算法：'round-robin'（轮询）或 'random'（随机）
    algorithm: process.env.LB_ALGORITHM || 'round-robin'
  },
  
  // 共享存储配置
  sharedStorage: {
    // 存储类型：'local'（本地存储）或 'nfs'（网络文件系统）
    type: process.env.STORAGE_TYPE || 'local',
    
    // 本地存储配置
    local: {
      path: process.env.ROOT_DIR || './storage'
    },
    
    // NFS存储配置
    nfs: {
      path: process.env.NFS_PATH || '/mnt/nfs/storage'
    }
  },
  
  // Redis配置（用于集群通信和状态管理）
  redis: {
    host: process.env.REDIS_HOST || 'localhost',
    port: process.env.REDIS_PORT || 6379,
    password: process.env.REDIS_PASSWORD || null
  }
};
