package main

import (
	"context"
	"encoding/hex"
	"fmt"
	"log/slog"
	"net"
	"os"
	"strings"

	pb "rs-relay-auth-server/proto"

	"github.com/nbd-wtf/go-nostr/nip19"
	"github.com/spf13/viper"
	"google.golang.org/grpc"
)

type Config struct {
	LogLevel       string   `mapstructure:"log_level"`
	ListenAddress  string   `mapstructure:"listen_address"`
	AllowedNpubs   []string `mapstructure:"allowed_npubs"`
}

type server struct {
	pb.UnimplementedAuthorizationServer
	allowedPubkeys map[string]bool
	logger         *slog.Logger
}

func loadConfig() (*Config, error) {
	viper.SetConfigName("policy-config")
	viper.SetConfigType("toml")
	viper.AddConfigPath(".")

	// Set defaults
	viper.SetDefault("log_level", "ERROR")
	viper.SetDefault("listen_address", "[::1]:50051")
	viper.SetDefault("allowed_npubs", []string{})

	if err := viper.ReadInConfig(); err != nil {
		return nil, fmt.Errorf("failed to read config: %w", err)
	}

	var config Config
	if err := viper.Unmarshal(&config); err != nil {
		return nil, fmt.Errorf("failed to unmarshal config: %w", err)
	}

	return &config, nil
}

func npubsToPubkeys(npubs []string) ([]string, error) {
	pubkeys := make([]string, 0, len(npubs))

	for _, npub := range npubs {
		prefix, value, err := nip19.Decode(npub)
		if err != nil {
			return nil, fmt.Errorf("failed to decode npub %s: %w", npub, err)
		}

		if prefix != "npub" {
			return nil, fmt.Errorf("expected npub prefix, got %s for %s", prefix, npub)
		}

		pubkeyHex := value.(string)
		pubkeys = append(pubkeys, pubkeyHex)
	}

	return pubkeys, nil
}

func newServer(config *Config, logger *slog.Logger) (*server, error) {
	pubkeys, err := npubsToPubkeys(config.AllowedNpubs)
	if err != nil {
		return nil, fmt.Errorf("failed to convert npubs to pubkeys: %w", err)
	}

	allowedMap := make(map[string]bool)
	for _, pk := range pubkeys {
		allowedMap[pk] = true
	}

	return &server{
		allowedPubkeys: allowedMap,
		logger:         logger,
	}, nil
}

func (s *server) EventAdmit(ctx context.Context, req *pb.EventRequest) (*pb.EventReply, error) {
	if req.AuthPubkey == nil || len(req.AuthPubkey) == 0 {
		s.logger.Info("DENY: No NIP-42 authentication")
		return &pb.EventReply{
			Decision: pb.Decision_DECISION_DENY,
			Message:  strPtr("auth-required: NIP-42 authentication required"),
		}, nil
	}

	if req.Event == nil {
		s.logger.Info("DENY: No event provided")
		return &pb.EventReply{
			Decision: pb.Decision_DECISION_DENY,
			Message:  strPtr("blocked: No event provided"),
		}, nil
	}

	authPubkeyHex := hex.EncodeToString(req.AuthPubkey)

	if !s.allowedPubkeys[authPubkeyHex] {
		s.logger.Info("DENY: Authenticated pubkey not allowed", "pubkey", authPubkeyHex)
		return &pb.EventReply{
			Decision: pb.Decision_DECISION_DENY,
			Message:  strPtr("Your pubkey is not authorized to publish"),
		}, nil
	}

	eventPubkeyHex := hex.EncodeToString(req.Event.Pubkey)
	s.logger.Info("PERMIT: Publishing event",
		"auth_pubkey", authPubkeyHex,
		"event_pubkey", eventPubkeyHex,
		"kind", req.Event.Kind)

	return &pb.EventReply{
		Decision: pb.Decision_DECISION_PERMIT,
		Message:  nil,
	}, nil
}

func strPtr(s string) *string {
	return &s
}

func parseLogLevel(level string) slog.Level {
	switch strings.ToUpper(level) {
	case "DEBUG":
		return slog.LevelDebug
	case "INFO":
		return slog.LevelInfo
	case "WARN":
		return slog.LevelWarn
	case "ERROR":
		return slog.LevelError
	default:
		return slog.LevelError
	}
}

func main() {
	config, err := loadConfig()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to load config: %v\n", err)
		os.Exit(1)
	}

	logLevel := parseLogLevel(config.LogLevel)
	logger := slog.New(slog.NewTextHandler(os.Stdout, &slog.HandlerOptions{
		Level: logLevel,
	}))

	srv, err := newServer(config, logger)
	if err != nil {
		logger.Error("Failed to create server", "error", err)
		os.Exit(1)
	}

	lis, err := net.Listen("tcp", config.ListenAddress)
	if err != nil {
		logger.Error("Failed to listen", "error", err)
		os.Exit(1)
	}

	grpcServer := grpc.NewServer()
	pb.RegisterAuthorizationServer(grpcServer, srv)

	fmt.Printf("NIP-42 Authorization Server listening on %s\n", config.ListenAddress)

	if err := grpcServer.Serve(lis); err != nil {
		logger.Error("Failed to serve", "error", err)
		os.Exit(1)
	}
}
