package main

import (
	"context"
	"encoding/hex"
	"log/slog"
	"os"
	"testing"

	pb "rs-relay-auth-server/proto"
)

func TestNpubsToPubkeys(t *testing.T) {
	tests := []struct {
		name    string
		npubs   []string
		want    []string
		wantErr bool
	}{
		{
			name: "valid single npub",
			npubs: []string{
				"npub1mkq63wkt4v94cvq869njlwpszwpmf62c84p3sdvc2ptjy04jnzjs20r4tx",
			},
			want: []string{
				"dd81a8bacbab0b5c3007d1672fb8301383b4e9583d431835985057223eb298a5",
			},
			wantErr: false,
		},
		{
			name: "valid multiple npubs",
			npubs: []string{
				"npub1mkq63wkt4v94cvq869njlwpszwpmf62c84p3sdvc2ptjy04jnzjs20r4tx",
				"npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6",
			},
			want: []string{
				"dd81a8bacbab0b5c3007d1672fb8301383b4e9583d431835985057223eb298a5",
				"3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d",
			},
			wantErr: false,
		},
		{
			name:    "empty list",
			npubs:   []string{},
			want:    []string{},
			wantErr: false,
		},
		{
			name: "invalid bech32",
			npubs: []string{
				"invalid",
			},
			want:    nil,
			wantErr: true,
		},
		{
			name: "wrong prefix (nsec instead of npub)",
			npubs: []string{
				"nsec1vl029mgpspedva04g90vltkh6fvh240zqtv9k0t9af8935ke9laqsnlfe5",
			},
			want:    nil,
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := npubsToPubkeys(tt.npubs)
			if (err != nil) != tt.wantErr {
				t.Errorf("npubsToPubkeys() error = %v, wantErr %v", err, tt.wantErr)
				return
			}
			if !tt.wantErr {
				if len(got) != len(tt.want) {
					t.Errorf("npubsToPubkeys() got %d pubkeys, want %d", len(got), len(tt.want))
					return
				}
				for i := range got {
					if got[i] != tt.want[i] {
						t.Errorf("npubsToPubkeys() got[%d] = %v, want %v", i, got[i], tt.want[i])
					}
				}
			}
		})
	}
}

func TestParseLogLevel(t *testing.T) {
	tests := []struct {
		name  string
		level string
		want  slog.Level
	}{
		{
			name:  "DEBUG lowercase",
			level: "debug",
			want:  slog.LevelDebug,
		},
		{
			name:  "DEBUG uppercase",
			level: "DEBUG",
			want:  slog.LevelDebug,
		},
		{
			name:  "INFO mixed case",
			level: "InFo",
			want:  slog.LevelInfo,
		},
		{
			name:  "WARN",
			level: "WARN",
			want:  slog.LevelWarn,
		},
		{
			name:  "ERROR",
			level: "ERROR",
			want:  slog.LevelError,
		},
		{
			name:  "invalid defaults to ERROR",
			level: "invalid",
			want:  slog.LevelError,
		},
		{
			name:  "empty defaults to ERROR",
			level: "",
			want:  slog.LevelError,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := parseLogLevel(tt.level)
			if got != tt.want {
				t.Errorf("parseLogLevel() = %v, want %v", got, tt.want)
			}
		})
	}
}

func TestEventAdmit(t *testing.T) {
	logger := slog.New(slog.NewTextHandler(os.Stdout, &slog.HandlerOptions{
		Level: slog.LevelError,
	}))

	allowedPubkeyHex := "dd81a8bacbab0b5c3007d1672fb8301383b4e9583d431835985057223eb298a5"
	allowedPubkeyBytes, _ := hex.DecodeString(allowedPubkeyHex)

	disallowedPubkeyHex := "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d"
	disallowedPubkeyBytes, _ := hex.DecodeString(disallowedPubkeyHex)

	eventPubkeyHex := "0000000000000000000000000000000000000000000000000000000000000001"
	eventPubkeyBytes, _ := hex.DecodeString(eventPubkeyHex)

	srv := &server{
		allowedPubkeys: map[string]bool{
			allowedPubkeyHex: true,
		},
		logger: logger,
	}

	tests := []struct {
		name         string
		req          *pb.EventRequest
		wantDecision pb.Decision
		wantMessage  bool
	}{
		{
			name: "no auth pubkey - deny",
			req: &pb.EventRequest{
				AuthPubkey: nil,
				Event: &pb.Event{
					Pubkey: eventPubkeyBytes,
					Kind:   1,
				},
			},
			wantDecision: pb.Decision_DECISION_DENY,
			wantMessage:  true,
		},
		{
			name: "allowed pubkey publishing own event - permit",
			req: &pb.EventRequest{
				AuthPubkey: allowedPubkeyBytes,
				Event: &pb.Event{
					Pubkey: allowedPubkeyBytes,
					Kind:   1,
				},
			},
			wantDecision: pb.Decision_DECISION_PERMIT,
			wantMessage:  false,
		},
		{
			name: "allowed pubkey publishing event signed by different pubkey - permit",
			req: &pb.EventRequest{
				AuthPubkey: allowedPubkeyBytes,
				Event: &pb.Event{
					Pubkey: eventPubkeyBytes,
					Kind:   1,
				},
			},
			wantDecision: pb.Decision_DECISION_PERMIT,
			wantMessage:  false,
		},
		{
			name: "allowed pubkey publishing event signed by disallowed pubkey - permit",
			req: &pb.EventRequest{
				AuthPubkey: allowedPubkeyBytes,
				Event: &pb.Event{
					Pubkey: disallowedPubkeyBytes,
					Kind:   1,
				},
			},
			wantDecision: pb.Decision_DECISION_PERMIT,
			wantMessage:  false,
		},
		{
			name: "disallowed pubkey - deny",
			req: &pb.EventRequest{
				AuthPubkey: disallowedPubkeyBytes,
				Event: &pb.Event{
					Pubkey: eventPubkeyBytes,
					Kind:   1,
				},
			},
			wantDecision: pb.Decision_DECISION_DENY,
			wantMessage:  true,
		},
		{
			name: "nil event - deny",
			req: &pb.EventRequest{
				AuthPubkey: allowedPubkeyBytes,
				Event:      nil,
			},
			wantDecision: pb.Decision_DECISION_DENY,
			wantMessage:  true,
		},
		{
			name: "empty auth pubkey bytes - deny",
			req: &pb.EventRequest{
				AuthPubkey: []byte{},
				Event: &pb.Event{
					Pubkey: eventPubkeyBytes,
					Kind:   1,
				},
			},
			wantDecision: pb.Decision_DECISION_DENY,
			wantMessage:  true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := srv.EventAdmit(context.Background(), tt.req)
			if err != nil {
				t.Errorf("EventAdmit() error = %v", err)
				return
			}
			if got.Decision != tt.wantDecision {
				t.Errorf("EventAdmit() decision = %v, want %v", got.Decision, tt.wantDecision)
			}
			if tt.wantMessage && got.Message == nil {
				t.Error("EventAdmit() expected message, got nil")
			}
			if !tt.wantMessage && got.Message != nil {
				t.Errorf("EventAdmit() expected no message, got %v", *got.Message)
			}
		})
	}
}

func TestNewServer(t *testing.T) {
	logger := slog.New(slog.NewTextHandler(os.Stdout, &slog.HandlerOptions{
		Level: slog.LevelError,
	}))

	tests := []struct {
		name    string
		config  *Config
		wantErr bool
		wantLen int
	}{
		{
			name: "valid single npub",
			config: &Config{
				AllowedNpubs: []string{
					"npub1mkq63wkt4v94cvq869njlwpszwpmf62c84p3sdvc2ptjy04jnzjs20r4tx",
				},
			},
			wantErr: false,
			wantLen: 1,
		},
		{
			name: "valid multiple npubs",
			config: &Config{
				AllowedNpubs: []string{
					"npub1mkq63wkt4v94cvq869njlwpszwpmf62c84p3sdvc2ptjy04jnzjs20r4tx",
					"npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6",
				},
			},
			wantErr: false,
			wantLen: 2,
		},
		{
			name: "empty list",
			config: &Config{
				AllowedNpubs: []string{},
			},
			wantErr: false,
			wantLen: 0,
		},
		{
			name: "invalid npub",
			config: &Config{
				AllowedNpubs: []string{
					"invalid",
				},
			},
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := newServer(tt.config, logger)
			if (err != nil) != tt.wantErr {
				t.Errorf("newServer() error = %v, wantErr %v", err, tt.wantErr)
				return
			}
			if !tt.wantErr {
				if len(got.allowedPubkeys) != tt.wantLen {
					t.Errorf("newServer() allowedPubkeys length = %v, want %v", len(got.allowedPubkeys), tt.wantLen)
				}
			}
		})
	}
}
