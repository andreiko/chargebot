# Chargebot

This is a Telegram bot that can monitor your favourite EV charger stations and notify you of availability changes. 

## Demo

https://github.com/andreiko/chargebot/assets/340586/9ad2bc49-f1f2-4e83-b151-819e9e2a8be9

## Architecture

Chargebot is an async program written in Rust using Tokio and around of the idea of message passing between 4 main components: 

![Architecture diagram](docs/architecture.png)
