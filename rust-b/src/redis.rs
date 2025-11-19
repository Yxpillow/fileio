use redis::AsyncCommands;

pub async fn set_key(url: &str, key: &str, value: &str) -> anyhow::Result<()> {
    let client = redis::Client::open(url)?;
    let mut conn = client.get_async_connection().await?;
    conn.set::<_, _, ()>(key, value).await?;
    Ok(())
}

pub async fn get_key(url: &str, key: &str) -> anyhow::Result<Option<String>> {
    let client = redis::Client::open(url)?;
    let mut conn = client.get_async_connection().await?;
    let res: Option<String> = conn.get(key).await?;
    Ok(res)
}

pub async fn del_key(url: &str, key: &str) -> anyhow::Result<()> {
    let client = redis::Client::open(url)?;
    let mut conn = client.get_async_connection().await?;
    let _: () = conn.del(key).await?;
    Ok(())
}

pub async fn register_node(url: &str, node_json: &str) -> anyhow::Result<()> {
    let client = redis::Client::open(url)?;
    let mut conn = client.get_async_connection().await?;
    let _: () = conn.sadd("nodes", node_json).await?;
    Ok(())
}

pub async fn list_nodes(url: &str) -> anyhow::Result<Vec<String>> {
    let client = redis::Client::open(url)?;
    let mut conn = client.get_async_connection().await?;
    let members: Vec<String> = conn.smembers("nodes").await?;
    Ok(members)
}