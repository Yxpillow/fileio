// 一致性哈希算法实现

class ConsistentHash {
  constructor(numReplicas = 10) {
    this.numReplicas = numReplicas; // 虚拟节点数量
    this.ring = {}; // 哈希环
    this.keys = []; // 已排序的哈希值
  }

  // 添加服务器节点
  addNode(node) {
    for (let i = 0; i < this.numReplicas; i++) {
      const replicaKey = `${node.id}:${i}`;
      const hash = this.getHash(replicaKey);
      this.ring[hash] = node;
      this.keys.push(hash);
    }
    // 对哈希值进行排序
    this.keys.sort((a, b) => a - b);
  }

  // 删除服务器节点
  removeNode(node) {
    for (let i = 0; i < this.numReplicas; i++) {
      const replicaKey = `${node.id}:${i}`;
      const hash = this.getHash(replicaKey);
      delete this.ring[hash];
      this.keys = this.keys.filter(key => key !== hash);
    }
  }

  // 获取文件应该存储的节点
  getNode(key) {
    if (this.keys.length === 0) {
      return null;
    }

    const hash = this.getHash(key);
    let index = this.keys.findIndex(h => h >= hash);

    // 如果没有找到大于等于哈希值的节点，使用第一个节点
    if (index === -1) {
      index = 0;
    }

    return this.ring[this.keys[index]];
  }

  // 获取所有节点
  getAllNodes() {
    const nodes = new Set();
    for (const hash in this.ring) {
      nodes.add(this.ring[hash]);
    }
    return Array.from(nodes);
  }

  // 哈希函数
  getHash(key) {
    let hash = 0;
    for (let i = 0; i < key.length; i++) {
      const char = key.charCodeAt(i);
      hash = ((hash << 5) - hash) + char;
      hash = hash & hash; // 转换为32位整数
    }
    return hash;
  }
}

module.exports = ConsistentHash;
