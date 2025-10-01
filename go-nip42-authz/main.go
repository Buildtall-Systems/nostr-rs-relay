package main

import (
	"context"
	"encoding/hex"
	"log"
	"net"

	pb "rs-relay-auth-server/proto"

	"google.golang.org/grpc"
)

type server struct {
	pb.UnimplementedAuthorizationServer
	allowedPubkeys map[string]bool
}

func newServer() *server {
	allowedPubkeys := []string{
		"dd81a8bacbab0b5c3007d1672fb8301383b4e9583d431835985057223eb298a5",
	}

	allowedMap := make(map[string]bool)
	for _, pk := range allowedPubkeys {
		allowedMap[pk] = true
	}

	return &server{
		allowedPubkeys: allowedMap,
	}
}

func (s *server) EventAdmit(ctx context.Context, req *pb.EventRequest) (*pb.EventReply, error) {
	if req.AuthPubkey == nil {
		log.Println("DENY: No NIP-42 authentication")
		return &pb.EventReply{
			Decision: pb.Decision_DECISION_DENY,
			Message:  strPtr("auth-required: NIP-42 authentication required"),
		}, nil
	}

	authPubkeyHex := hex.EncodeToString(req.AuthPubkey)

	if !s.allowedPubkeys[authPubkeyHex] {
		log.Printf("DENY: Authenticated pubkey not allowed: %s\n", authPubkeyHex)
		return &pb.EventReply{
			Decision: pb.Decision_DECISION_DENY,
			Message:  strPtr("Your pubkey is not authorized to publish"),
		}, nil
	}

	eventPubkeyHex := hex.EncodeToString(req.Event.Pubkey)
	log.Printf("PERMIT: Authenticated pubkey %s publishing event from author %s (kind=%d)\n",
		authPubkeyHex, eventPubkeyHex, req.Event.Kind)

	return &pb.EventReply{
		Decision: pb.Decision_DECISION_PERMIT,
		Message:  nil,
	}, nil
}

func strPtr(s string) *string {
	return &s
}

func main() {
	lis, err := net.Listen("tcp", "[::1]:50051")
	if err != nil {
		log.Fatalf("Failed to listen: %v", err)
	}

	grpcServer := grpc.NewServer()
	pb.RegisterAuthorizationServer(grpcServer, newServer())

	log.Println("NIP-42 Authorization Server listening on :50051")
	if err := grpcServer.Serve(lis); err != nil {
		log.Fatalf("Failed to serve: %v", err)
	}
}
