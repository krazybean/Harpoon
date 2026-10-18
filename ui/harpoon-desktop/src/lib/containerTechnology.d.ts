export type ContainerTechnologyKey = "postgres" | "redis" | "nginx" | "node" | "mysql" | "mariadb" | "mongo" | "rabbitmq" | "traefik" | "caddy" | "python" | "alpine" | "unknown";
export type ContainerTechnology = { key: ContainerTechnologyKey; label: string };
export function inferContainerTechnology(container: any): ContainerTechnology;
